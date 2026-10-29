"""Small, independent OOXML operation runtime. Python 3.9+, no native engine."""
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import tempfile
from xml.dom import minidom
from zipfile import ZipFile, ZIP_DEFLATED

W = 'http://schemas.openxmlformats.org/wordprocessingml/2006/main'
CHAR = {'bold': 'b', 'boldCs': 'bCs', 'italic': 'i', 'italicCs': 'iCs',
        'strike': 'strike', 'doubleStrike': 'dstrike', 'caps': 'caps',
        'smallCaps': 'smallCaps', 'hidden': 'vanish', 'underline': 'u',
        'size': 'sz', 'sizeCs': 'szCs', 'color': 'color', 'highlight': 'highlight',
        'style': 'rStyle', 'vertAlign': 'vertAlign', 'spacing': 'spacing',
        'font': 'rFonts', 'fontEa': 'rFonts', 'fontCs': 'rFonts'}
BOOL = {'bold', 'boldCs', 'italic', 'italicCs', 'strike', 'doubleStrike', 'caps', 'smallCaps', 'hidden'}
PARA = {'style': 'pStyle', 'align': 'jc', 'keepNext': 'keepNext',
        'keepLines': 'keepLines', 'pageBreakBefore': 'pageBreakBefore',
        'spaceBefore': 'spacing', 'spaceAfter': 'spacing',
        'indentLeft': 'ind', 'indentRight': 'ind', 'indentFirst': 'ind'}


def children(node, name=None):
    return [n for n in node.childNodes if n.nodeType == n.ELEMENT_NODE
            and (name is None or (n.namespaceURI == W and n.localName == name))]


def child(node, name):
    return next(iter(children(node, name)), None)


def ensure(node, name, first=False):
    item = child(node, name)
    if item is None:
        item = node.ownerDocument.createElementNS(W, 'w:' + name)
        node.insertBefore(item, node.firstChild) if first else node.appendChild(item)
    return item


def val(node, key, value):
    node.setAttributeNS(W, 'w:' + key, str(value))


def text_of(node):
    out = []
    for n in node.childNodes:
        if n.nodeType != n.ELEMENT_NODE:
            continue
        if n.namespaceURI == W and n.localName == 't':
            out.append(''.join(x.data for x in n.childNodes if x.nodeType in (x.TEXT_NODE, x.CDATA_SECTION_NODE)))
        elif n.namespaceURI == W and n.localName == 'tab':
            out.append('\t')
        elif n.namespaceURI == W and n.localName in ('br', 'cr'):
            out.append('\n')
        elif n.namespaceURI == W and n.localName in ('drawing', 'pict', 'object'):
            out.append('\ufffc')
        else:
            out.append(text_of(n))
    return ''.join(out)


def set_run_text(run, text):
    for n in list(run.childNodes):
        if not (n.nodeType == n.ELEMENT_NODE and n.namespaceURI == W and n.localName == 'rPr'):
            run.removeChild(n)
    # Preserve tabs and line breaks as OOXML elements, not literal control characters.
    chunk = ''
    for c in text + '\0':
        if c in '\t\n\0':
            if chunk:
                t = run.ownerDocument.createElementNS(W, 'w:t')
                t.setAttribute('xml:space', 'preserve')
                t.appendChild(run.ownerDocument.createTextNode(chunk))
                run.appendChild(t)
                chunk = ''
            if c != '\0':
                run.appendChild(run.ownerDocument.createElementNS(W, 'w:tab' if c == '\t' else 'w:br'))
        else:
            chunk += c


def plain_runs(p):
    result=[]
    for n in children(p):
        if n.namespaceURI != W or n.localName not in ('pPr', 'r', 'hyperlink'):
            raise ValueError('This paragraph contains a hyperlink, revision, bookmark or field; compact text editing is not supported here yet')
        if n.localName == 'hyperlink':
            result.extend(plain_runs(n))
        if n.localName == 'r' and any(x.namespaceURI != W or x.localName not in ('rPr', 't', 'tab', 'br', 'cr', 'drawing') for x in children(n)):
            raise ValueError('This paragraph contains an object or field; compact text editing is not supported here yet')
        if n.localName == 'r': result.append(n)
    return result


def split_at(p, offset):
    runs = plain_runs(p)
    total = sum(len(text_of(r)) for r in runs)
    if not isinstance(offset, int) or offset < 0 or offset > total:
        raise ValueError('Text range is outside the paragraph')
    pos = 0
    for r in runs:
        text = text_of(r)
        if pos < offset < pos + len(text):
            right = r.cloneNode(True)
            set_run_text(r, text[:offset-pos])
            set_run_text(right, text[offset-pos:])
            r.parentNode.insertBefore(right, r.nextSibling)
            break
        pos += len(text)


def format_props(rpr, props):
    props = dict(props)
    for key in ('bold', 'italic', 'size'):
        if key in props and key+'Cs' not in props: props[key+'Cs'] = props[key]
    unknown = set(props) - set(CHAR)
    if unknown:
        raise ValueError('Unsupported character properties: ' + ', '.join(sorted(unknown)))
    for key, value in props.items():
        tag = CHAR[key]
        item = child(rpr, tag)
        if key in ('font', 'fontEa', 'fontCs'):
            attrs = {'font': ('ascii', 'hAnsi'), 'fontEa': ('eastAsia',), 'fontCs': ('cs',)}[key]
            item = ensure(rpr, tag)
            for a in attrs:
                if value is None:
                    if item.hasAttributeNS(W, a): item.removeAttributeNS(W, a)
                else: val(item, a, value)
            continue
        if value is None:
            if item is not None: rpr.removeChild(item)
            continue
        item = ensure(rpr, tag)
        if key in BOOL: value = '1' if value else '0'
        elif key in ('size', 'sizeCs'): value = round(value * 2)
        elif key == 'spacing': value = round(value * 20)
        elif key == 'color':
            if isinstance(value, dict): value = value.get('Rgb', 'auto')
            if isinstance(value, list): value = ''.join(f'{v:02X}' for v in value)
            if str(value).lower() == 'auto': value = 'auto'
        val(item, 'val', value)


class Editor:
    """Targets are bound once to actual XML nodes, so indices do not drift after edits."""
    def __init__(self, input_path, *, targets=None, source_sha256=None):
        self.input = Path(input_path).expanduser().resolve(strict=True)
        if source_sha256 and hashlib.sha256(self.input.read_bytes()).hexdigest() != source_sha256:
            raise ValueError('Input does not match the recorded original')
        with ZipFile(self.input) as archive:
            if sum(i.file_size for i in archive.infolist()) > 256 * 1024 * 1024:
                raise ValueError('DOCX expands beyond 256 MiB')
            self.parts = {n: archive.read(n) for n in archive.namelist()}
        raw = self.parts['word/document.xml']
        if b'<!DOCTYPE' in raw.upper() or b'<!ENTITY' in raw.upper():
            raise ValueError('DTD/entity declarations are not accepted')
        self.xml = minidom.parseString(raw)
        self.body = self.xml.getElementsByTagNameNS(W, 'body')[0]
        self.targets = {}
        if isinstance(targets, (str, Path)):
            targets = json.loads(Path(targets).read_text(encoding='utf-8'))
        for name, spec in (targets or {}).items():
            node = self.body
            for index in spec['path']:
                blocks = [n for n in children(node) if n.namespaceURI == W and n.localName in ('p', 'tbl', 'tr', 'tc')]
                node = blocks[index]
            if spec.get('text') is not None and text_of(node) != spec['text']:
                raise ValueError('Target text mismatch: ' + name)
            if spec.get('textSha256') and hashlib.sha256(text_of(node).encode()).hexdigest() != spec['textSha256']:
                raise ValueError('Target text mismatch: ' + name)
            self.targets[name] = node

    def replace_text(self, target, start, end, text):
        p = self.targets[target]
        if '\ufffc' in text: raise ValueError('Use a picture operation for embedded objects')
        if start > end: raise ValueError('Invalid range')
        split_at(p, end)
        split_at(p, start)
        runs = plain_runs(p)
        pos, selected, anchor, template = 0, [], None, None
        for r in runs:
            length = len(text_of(r))
            if pos <= start: template = r
            if pos == start: anchor = r
            if start <= pos < end: selected.append(r)
            pos += length
        if text:
            r = template.cloneNode(True) if template is not None else self.xml.createElementNS(W, 'w:r')
            set_run_text(r, text)
            anchor.parentNode.insertBefore(r, anchor) if anchor is not None else p.appendChild(r)
        for r in selected: r.parentNode.removeChild(r)

    def format_text(self, target, start, end, **props):
        p = self.targets[target]
        if start > end: raise ValueError('Invalid range')
        split_at(p, end)
        split_at(p, start)
        pos = 0
        for r in plain_runs(p):
            length = len(text_of(r))
            if start <= pos < end: format_props(ensure(r, 'rPr', first=True), props)
            pos += length

    def format_mark(self, target, **props):
        format_props(ensure(ensure(self.targets[target], 'pPr', first=True), 'rPr'), props)

    def format_paragraph(self, target, **props):
        if set(props) - set(PARA): raise ValueError('Unsupported paragraph properties')
        ppr = ensure(self.targets[target], 'pPr', first=True)
        for key, value in props.items():
            tag = PARA[key]
            item = child(ppr, tag)
            attr = {'spaceBefore':'before', 'spaceAfter':'after', 'indentLeft':'left', 'indentRight':'right', 'indentFirst':'firstLine'}.get(key, 'val')
            if value is None:
                if item is not None:
                    if attr == 'val': ppr.removeChild(item)
                    elif item.hasAttributeNS(W, attr): item.removeAttributeNS(W, attr)
                continue
            item = ensure(ppr, tag)
            if key in ('keepNext', 'keepLines', 'pageBreakBefore'): value = '1' if value else '0'
            elif attr != 'val': value = round(value * 20)
            val(item, attr, value)

    def insert_paragraph(self, target, after, text=''):
        p = self.xml.createElementNS(W, 'w:p')
        if after is None:
            self.body.insertBefore(p, self.body.firstChild)
        else:
            anchor = self.targets[after]
            anchor.parentNode.insertBefore(p, anchor.nextSibling)
        self.targets[target] = p
        self.replace_text(target, 0, 0, text)

    def delete_block(self, target):
        node = self.targets.pop(target)
        node.parentNode.removeChild(node)

    def set_cell(self, target, row, col, text):
        table = self.targets[target]
        cell = children(children(table, 'tr')[row], 'tc')[col]
        blocks = [n for n in children(cell) if n.localName in ('p', 'tbl')]
        if len(blocks) != 1 or blocks[0].localName != 'p':
            raise ValueError('set_cell requires a single plain paragraph')
        name = target + f'/r{row}/c{col}'
        self.targets[name] = blocks[0]
        self.replace_text(name, 0, len(text_of(blocks[0])), text)

    def insert_table(self, target, after, rows, cols, widths, style='TableGrid'):
        if not 1 <= rows <= 1000 or not 1 <= cols <= 100 or len(widths)!=cols:
            raise ValueError('Invalid table dimensions')
        table=self.xml.createElementNS(W,'w:tbl')
        pr=ensure(table,'tblPr')
        val(ensure(pr,'tblStyle'),'val',style)
        look=ensure(pr,'tblLook')
        for k,v in {'firstRow':'1','lastRow':'0','firstColumn':'1','lastColumn':'0','noHBand':'0','noVBand':'1'}.items(): val(look,k,v)
        grid=ensure(table,'tblGrid')
        for width in widths:
            col=self.xml.createElementNS(W,'w:gridCol');val(col,'w',round(width*20));grid.appendChild(col)
        for _ in range(rows):
            row=self.xml.createElementNS(W,'w:tr');table.appendChild(row)
            for width in widths:
                cell=self.xml.createElementNS(W,'w:tc');row.appendChild(cell)
                tcw=ensure(ensure(cell,'tcPr'),'tcW');val(tcw,'type','dxa');val(tcw,'w',round(width*20))
                cell.appendChild(self.xml.createElementNS(W,'w:p'))
        if after is None: self.body.insertBefore(table,self.body.firstChild)
        else:
            anchor=self.targets[after];anchor.parentNode.insertBefore(table,anchor.nextSibling)
        self.targets[target]=table

    def insert_picture(self, target, offset, path, width, height, alt=''):
        p=self.targets[target]
        split_at(p,offset)
        ext=Path(path).suffix.lower().lstrip('.')
        mime={'png':'image/png','jpg':'image/jpeg','jpeg':'image/jpeg','gif':'image/gif'}.get(ext)
        if mime is None: raise ValueError('Only PNG, JPEG and GIF are supported')
        data=Path(path).read_bytes()
        if len(data)>32*1024*1024: raise ValueError('Picture exceeds 32 MiB')
        digest=hashlib.sha256(data).hexdigest()[:16]
        name=f'trace-{digest}.{ext}'
        self.parts['word/media/'+name]=data
        relns='http://schemas.openxmlformats.org/package/2006/relationships'
        relpart='word/_rels/document.xml.rels'
        rel=minidom.parseString(self.parts.get(relpart,f'<Relationships xmlns="{relns}"/>'.encode()))
        ids={e.getAttribute('Id') for e in rel.documentElement.childNodes if e.nodeType==e.ELEMENT_NODE}
        rid='rIdTrace'+digest
        while rid in ids: rid+='x'
        item=rel.createElementNS(relns,'Relationship')
        for k,v in {'Id':rid,'Type':'http://schemas.openxmlformats.org/officeDocument/2006/relationships/image','Target':'media/'+name}.items():item.setAttribute(k,v)
        rel.documentElement.appendChild(item);self.parts[relpart]=rel.toxml(encoding='UTF-8')
        types=minidom.parseString(self.parts['[Content_Types].xml'])
        if not any(e.getAttribute('Extension')==ext for e in types.documentElement.childNodes if e.nodeType==e.ELEMENT_NODE):
            e=types.createElementNS(types.documentElement.namespaceURI,'Default');e.setAttribute('Extension',ext);e.setAttribute('ContentType',mime);types.documentElement.appendChild(e)
        self.parts['[Content_Types].xml']=types.toxml(encoding='UTF-8')
        cx,cy=round(width*12700),round(height*12700)
        # Original DrawingML markup, generated from the public OOXML element vocabulary.
        xml=f'''<w:r xmlns:w="{W}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="1" name="Picture"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="0" name="Picture"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>'''
        r=self.xml.importNode(minidom.parseString(xml).documentElement,True)
        docpr=r.getElementsByTagNameNS('http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing','docPr')[0]
        used=[int(e.getAttribute('id')) for e in self.xml.getElementsByTagNameNS('http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing','docPr') if e.getAttribute('id').isdigit()]
        docpr.setAttribute('id',str(max(used,default=0)+1));docpr.setAttribute('descr',alt)
        pos=0
        for run in plain_runs(p):
            if pos==offset:
                run.parentNode.insertBefore(r,run);return
            pos+=len(text_of(run))
        p.appendChild(r)

    def set_page(self, **props):
        allowed = {'pageW','pageH','marginTop','marginBottom','marginLeft','marginRight','header','footer','gutter','landscape'}
        if set(props) - allowed: raise ValueError('Unsupported page properties')
        section = ensure(self.body, 'sectPr')
        for key, value in props.items():
            if key in ('pageW','pageH','landscape'):
                item = ensure(section, 'pgSz')
                attr = {'pageW':'w','pageH':'h','landscape':'orient'}[key]
                value = ('landscape' if value else 'portrait') if key == 'landscape' else round(value*20)
            else:
                item = ensure(section, 'pgMar')
                attr = key.removeprefix('margin').lower()
                value = round(value*20)
            val(item, attr, value)

    def save(self, output_path=None):
        output = Path(output_path) if output_path else self.input.with_name(self.input.stem + '_replayed.docx')
        output = output.expanduser().absolute()
        if output.exists() or output.resolve() == self.input:
            raise ValueError('Choose a new output path')
        parts = dict(self.parts)
        parts['word/document.xml'] = self.xml.toxml(encoding='UTF-8')
        fd, temp = tempfile.mkstemp(prefix='.docx-ops-', suffix='.docx', dir=output.parent)
        os.close(fd)
        try:
            with ZipFile(temp, 'w', ZIP_DEFLATED) as archive:
                for name, content in parts.items(): archive.writestr(name, content)
            # Hard-link publishes atomically and never overwrites an existing destination.
            os.link(temp, output)
        finally:
            os.unlink(temp)
        return str(output)
