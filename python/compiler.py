"""Compile a WordCraft before/after model into compact, explicit editing operations.

No model snapshot, XML document, or native engine is hidden in the generated program.
Unsupported changed features fail closed. Unchanged features are left in the input DOCX.
"""
from copy import deepcopy
from difflib import SequenceMatcher
import json
import hashlib
import sys
from docx_ops import CHAR, PARA, Editor


def delta(a, b):
    return {k: b.get(k) for k in sorted(set(a) | set(b)) if a.get(k) != b.get(k)}


def require_keys(props, allowed, context):
    unknown = set(props) - set(allowed)
    if unknown:
        raise ValueError(context + ': compact export does not yet support ' + ', '.join(sorted(unknown)))


def chars(para):
    text = para.get('text', '')
    encoded = text.encode('utf-8')
    out, offset = [], 0
    for run in para.get('runs', []):
        end = offset + run['len']
        piece = encoded[offset:end].decode('utf-8')
        out.extend([run.get('props', {})] * len(piece))
        offset = end
    if offset != len(encoded):
        if not encoded and not out: return []
        raise ValueError('Invalid UTF-8 run boundaries')
    return out


class Compiler:
    def __init__(self, base, final):
        self.base, self.final = base, final
        self.ops, self.targets, self.ids = [], {}, {}
        self.assets = {}
        self.counter = 0
        self.index(base.get('body', []), [])

    def index(self, blocks, path):
        for i, block in enumerate(blocks):
            here = path + [i]
            name = ('p' if block['kind'] == 'para' else 't') + str(len(self.ids))
            self.ids[tuple(here)] = name
            self.targets[name] = {'path': here}
            if block['kind'] == 'para':
                self.targets[name]['textSha256'] = hashlib.sha256(block.get('text','').encode()).hexdigest()
            elif block['kind'] == 'table':
                for r, row in enumerate(block['rows']):
                    for c, cell in enumerate(row['cells']):
                        self.index(cell['blocks'], here + [r, c])

    def emit(self, op, **args):
        self.ops.append({'op': op, **args})

    def paragraph(self, a, b, target):
        if b.get('objects') and not a.get('objects'):
            objects=b['objects']
            if any(o.get('type')!='image' or o.get('float',{}).get('wrap','inline')!='inline' or any(o.get('crop',[])) for o in objects):
                raise ValueError(target+': compact export supports newly inserted inline images only')
            if b.get('text','').count('\ufffc')!=len(objects): raise ValueError('Image placeholders do not match objects')
            simple=deepcopy(b);simple.pop('objects',None)
            text=b.get('text','');styles=chars(b)
            simple['text']=text.replace('\ufffc','')
            simple['runs']=[{'len':len(c.encode()),'props':p} for c,p in zip(text,styles) if c!='\ufffc']
            self.paragraph(a,simple,target)
            for offset,obj in zip([i for i,c in enumerate(text) if c=='\ufffc'],objects):
                media=obj['media'];ext=media.rsplit('.',1)[-1].lower()
                if ext not in ('png','jpg','jpeg','gif'): raise ValueError('Unsupported picture format: '+ext)
                asset=f'assets/image{len(self.assets)+1}.{ext}'
                self.assets[asset]=self.final['media'][media]
                self.emit('insert_picture',target=target,offset=offset,path=asset,width=obj['w'],height=obj['h'],alt=obj.get('alt',''))
            return
        extra = delta({k:v for k,v in a.items() if k not in ('text','runs','props','mark')},
                      {k:v for k,v in b.items() if k not in ('text','runs','props','mark')})
        if extra:
            raise ValueError(f'{target}: changed paragraph objects/fields require an OOXML adapter ({", ".join(extra)})')
        old, new = a.get('text',''), b.get('text','')
        before, after = chars(a), chars(b)
        if old != new:
            prefix = 0
            while prefix < min(len(old),len(new)) and old[prefix] == new[prefix]: prefix += 1
            suffix = 0
            while suffix < min(len(old),len(new))-prefix and old[len(old)-suffix-1] == new[len(new)-suffix-1]: suffix += 1
            end = len(old)-suffix
            inserted = new[prefix:len(new)-suffix]
            self.emit('replace_text', target=target, start=prefix, end=end, text=inserted)
            # The runtime copies the run at the insertion point (previous run at end).
            inherited = before[min(prefix,len(before)-1)] if before else {}
            before = before[:prefix] + [inherited] * len(inserted) + before[end:]
        differences = [delta(x,y) for x,y in zip(before,after)]
        i = 0
        while i < len(differences):
            props = differences[i]
            j = i+1
            while j < len(differences) and differences[j] == props: j += 1
            if props:
                require_keys(props, CHAR, target)
                for key in ('bold','italic','size'):
                    if key in props:
                        props[key+'Cs'] = after[i].get(key+'Cs')
                        if props[key+'Cs'] == props[key]: del props[key+'Cs']
                self.emit('format_text', target=target, start=i, end=j, **props)
            i = j
        props = delta(a.get('props',{}), b.get('props',{}))
        if props:
            require_keys(props, PARA, target)
            self.emit('format_paragraph', target=target, **props)
        mark = delta(a.get('mark',{}), b.get('mark',{}))
        if mark:
            require_keys(mark, CHAR, target)
            for key in ('bold','italic','size'):
                if key in mark:
                    mark[key+'Cs']=b.get('mark',{}).get(key+'Cs')
                    if mark[key+'Cs']==mark[key]: del mark[key+'Cs']
            self.emit('format_mark', target=target, **mark)

    def table(self, a, b, path):
        target = self.ids[tuple(path)]
        if delta({k:v for k,v in a.items() if k!='rows'}, {k:v for k,v in b.items() if k!='rows'}):
            raise ValueError(target + ': table geometry/style changes require an OOXML adapter')
        if len(a['rows']) != len(b['rows']): raise ValueError(target + ': row insertion/deletion is not supported by compact export yet')
        for r,(oldrow,newrow) in enumerate(zip(a['rows'],b['rows'])):
            if oldrow.get('props') != newrow.get('props') or len(oldrow['cells']) != len(newrow['cells']):
                raise ValueError(target + ': changed row geometry is not supported yet')
            for c,(old,new) in enumerate(zip(oldrow['cells'],newrow['cells'])):
                if old.get('props') != new.get('props'): raise ValueError(target + ': cell geometry/merging is not supported yet')
                self.blocks(old['blocks'],new['blocks'],path+[r,c],nested=True)

    def blocks(self, a, b, path, nested=False):
        def signature(x): return (x['kind'], x.get('text',''))
        matcher = SequenceMatcher(a=[signature(x) for x in a], b=[signature(x) for x in b], autojunk=False)
        previous = None
        for tag,i,j,k,l in matcher.get_opcodes():
            count = min(j-i,l-k)
            for n in range(count):
                old,new = a[i+n],b[k+n]
                name = self.ids[tuple(path+[i+n])]
                if old['kind'] != new['kind']: raise ValueError('Changing a block type is not supported yet')
                if old['kind'] == 'para': self.paragraph(old,new,name)
                elif old['kind'] == 'table': self.table(old,new,path+[i+n])
                elif old != new: raise ValueError('Unsupported block change: '+old['kind'])
                previous = name
            for n in range(i+count,j):
                self.emit('delete_block',target=self.ids[tuple(path+[n])])
            for n in range(k+count,l):
                if nested: raise ValueError('Adding blocks inside cells is not supported yet')
                new = b[n]
                self.counter += 1
                name = 'new'+str(self.counter)
                if new['kind']=='para':
                    self.emit('insert_paragraph',target=name,after=previous,text='')
                    self.paragraph({'kind':'para','text':'','runs':[]},new,name)
                elif new['kind']=='table':
                    rows=new['rows'];cols=len(new['grid'])
                    if any(len(row['cells'])!=cols for row in rows): raise ValueError('Merged new tables are not supported yet')
                    self.emit('insert_table',target=name,after=previous,rows=len(rows),cols=cols,widths=new['grid'],style=new.get('props',{}).get('style','TableGrid'))
                    for r,row in enumerate(rows):
                        for c,cell in enumerate(row['cells']):
                            blocks=cell['blocks']
                            if len(blocks)!=1 or blocks[0]['kind']!='para' or cell.get('props',{}).get('span',1)!=1:
                                raise ValueError('Complex new table cells are not supported yet')
                            if blocks[0].get('text'):self.emit('set_cell',target=name,row=r,col=c,text=blocks[0]['text'])
                            if any(run.get('props') for run in blocks[0].get('runs',[])) or blocks[0].get('props'):
                                raise ValueError('Styled new table content requires an additional adapter')
                else: raise ValueError('Inserting this object requires an OOXML adapter')
                previous = name

    def compile(self):
        # Save bookkeeping is not a user content edit; package metadata is preserved from input.
        bookkeeping = {'modified', 'revision', 'lastModifiedBy'}
        metadata = delta({k:v for k,v in self.base.get('core',{}).items() if k not in bookkeeping},
                         {k:v for k,v in self.final.get('core',{}).items() if k not in bookkeeping})
        if metadata:
            raise ValueError('Compact export cannot yet represent document metadata: '+', '.join(metadata))
        ignored = {'body','lastSection','core','media'}
        other = delta({k:v for k,v in self.base.items() if k not in ignored},
                      {k:v for k,v in self.final.items() if k not in ignored})
        if other:
            raise ValueError('Compact export cannot yet represent changes to: '+', '.join(other))
        self.blocks(self.base.get('body',[]),self.final.get('body',[]),[])
        # Assets are emitted only for explicitly represented image operations.
        oldmedia=self.base.get('media',{});newmedia=self.final.get('media',{})
        if any(newmedia.get(k)!=v for k,v in oldmedia.items()): raise ValueError('Replacing/removing existing image resources is not supported yet')
        added=[v for k,v in newmedia.items() if k not in oldmedia]
        if any(v not in self.assets.values() for v in added): raise ValueError('An image resource has no compact operation')
        page = delta(self.base.get('lastSection',{}),self.final.get('lastSection',{}))
        if page:
            require_keys(page, {'pageW','pageH','marginTop','marginBottom','marginLeft','marginRight','header','footer','gutter','landscape'}, 'page')
            self.emit('set_page',**page)
        used = {v for op in self.ops for k,v in op.items() if k in ('target','after') and isinstance(v,str)}
        targets = {k:v for k,v in self.targets.items() if k in used}
        return {'schema':1,'offsets':'unicode-codepoints','ops':self.ops,'targets':targets,'assets':self.assets}


def program(plan, source_sha256):
    lines = ['from pathlib import Path', 'from docx_ops import Editor', '',
             'HERE = Path(__file__).resolve().parent', '',
             'def transform(input_path, output_path=None):',
             f'    doc = Editor(input_path, targets=HERE / "targets.json", source_sha256={source_sha256!r})']
    for op in plan['ops']:
        args = ', '.join(f'{k}=HERE / {v!r}' if k=='path' else f'{k}={v!r}' for k,v in op.items() if k!='op')
        lines.append(f"    doc.{op['op']}({args})")
    lines.extend(['    return doc.save(output_path)', '', "if __name__ == '__main__':",
                  '    import argparse', '    parser = argparse.ArgumentParser()',
                  '    parser.add_argument("input_path")', '    parser.add_argument("output_path", nargs="?")',
                  '    args = parser.parse_args()', '    print(transform(args.input_path, args.output_path))',''])
    return '\n'.join(lines)


def run(job):
    if job.get('action') == 'targets':
        return {'targets':Compiler(job['base'],job['base']).targets,'offsets':'unicode-codepoints'}
    if job.get('action') == 'apply':
        allowed = {'replace_text','format_text','format_mark','format_paragraph','insert_paragraph','delete_block','set_cell','set_page','insert_table','insert_picture'}
        doc = Editor(job['input'], targets=Compiler(job['base'],job['base']).targets)
        if not isinstance(job['ops'],list) or len(job['ops'])>1000: raise ValueError('Expected at most 1000 operations')
        for op in job['ops']:
            if not isinstance(op,dict) or op.get('op') not in allowed: raise ValueError('Unknown semantic operation')
            getattr(doc,op['op'])(**{k:v for k,v in op.items() if k!='op'})
        return {'output':doc.save(job['output'])}
    result = Compiler(job['base'],job['final']).compile()
    result['code'] = program(result,job.get('source_sha256'))
    return result


if __name__ == '__main__':
    try:
        print(json.dumps(run(json.load(sys.stdin)),ensure_ascii=False))
    except (ValueError,KeyError,IndexError,TypeError,UnicodeError) as error:
        print(str(error),file=sys.stderr)
        sys.exit(1)
