import json
from pathlib import Path
import tempfile
import unittest
from zipfile import ZipFile
from xml.dom import minidom
from docx_ops import Editor, W, children, child, text_of
from compiler import Compiler, program


def paragraph(text, props=None):
    return {'kind':'para','text':text,'runs':[{'len':len(text.encode()),'props':props or {}}] if text else []}


class OpsTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory()
        self.root=Path(self.tmp.name)
        self.source=self.root/'input.docx'
        with ZipFile(self.source,'w') as z:
            z.writestr('word/document.xml',f'<w:document xmlns:w="{W}"><w:body><w:p><w:r><w:rPr><w:i/></w:rPr><w:t>ab世界😀cd</w:t></w:r></w:p><w:p><w:r><w:t>untouched</w:t></w:r></w:p><w:sectPr/></w:body></w:document>')
            z.writestr('custom/data.bin',b'preserve me exactly')

    def tearDown(self): self.tmp.cleanup()

    def editor(self): return Editor(self.source,targets={'p0':{'path':[0],'text':'ab世界😀cd'},'p1':{'path':[1],'text':'untouched'}})

    def test_unicode_range_preserves_outside_format_and_parts(self):
        doc=self.editor()
        doc.replace_text('p0',2,4,'你好')
        doc.format_text('p0',2,5,bold=True)
        self.assertEqual(text_of(doc.targets['p0']),'ab你好😀cd')
        runs=children(doc.targets['p0'],'r')
        self.assertEqual([text_of(r) for r in runs],['ab','你好','😀','cd'])
        self.assertTrue(all(child(child(r,'rPr'),'i') is not None for r in runs))
        self.assertIsNone(child(child(runs[0],'rPr'),'b'))
        self.assertIsNotNone(child(child(runs[1],'rPr'),'b'))
        output=doc.save(self.root/'output.docx')
        with ZipFile(output) as z: self.assertEqual(z.read('custom/data.bin'),b'preserve me exactly')

    def test_stable_targets_survive_insertion_and_deletion(self):
        doc=self.editor()
        doc.insert_paragraph('new','p0','inserted')
        doc.delete_block('p0')
        doc.replace_text('p1',0,9,'still correct')
        self.assertEqual([text_of(p) for p in children(doc.body,'p')],['inserted','still correct'])

    def test_refuses_wrong_input_and_existing_output(self):
        with self.assertRaises(ValueError): Editor(self.source,source_sha256='bad')
        doc=self.editor()
        with self.assertRaises(ValueError): doc.save(self.source)
        with self.assertRaises(ValueError): doc.format_text('p0',0,100,bold=True)
        with self.assertRaises(ValueError): doc.format_text('p0',0,1,unknown=True)

    def test_compiler_exports_one_replacement_not_typing_history(self):
        base={'body':[paragraph('ab世界😀cd')]}
        final={'body':[paragraph('ab你好😀cd')]}
        plan=Compiler(base,final).compile()
        self.assertEqual(plan['ops'],[{'op':'replace_text','target':'p0','start':2,'end':4,'text':'你好'}])
        self.assertNotIn('TRACE',program(plan,None))
        self.assertLess(len(program(plan,None).splitlines()),25)

    def test_unsupported_change_fails_explicitly(self):
        base={'body':[paragraph('x')]}
        final={'body':[paragraph('x',{'shadow':True})]}
        with self.assertRaisesRegex(ValueError,'shadow'): Compiler(base,final).compile()

    def test_code_literals_cannot_inject_python(self):
        text="'''\n__import__('os').system('bad') 😀"
        plan=Compiler({'body':[paragraph('')]},{'body':[paragraph(text)]}).compile()
        compile(program(plan,None),'replay.py','exec')
        self.assertEqual(plan['ops'][0]['text'],text)

    def test_metadata_edits_are_not_silently_dropped(self):
        base={'body':[paragraph('x')], 'core':{'title':'before','modified':'old'}}
        final={'body':[paragraph('x')], 'core':{'title':'after','modified':'new'}}
        with self.assertRaisesRegex(ValueError,'metadata'): Compiler(base,final).compile()
        final['core']['title']='before'
        self.assertEqual(Compiler(base,final).compile()['ops'],[])

    def test_no_changes_produce_no_ops(self):
        model={'body':[paragraph('unchanged')]}
        self.assertEqual(Compiler(model,model).compile()['ops'],[])


if __name__=='__main__': unittest.main()
