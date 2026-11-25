import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from zipfile import ZipFile


class SlicerPackageDiffTests(unittest.TestCase):
    def test_detects_grid_and_anchor_mode_changes(self):
        def package(path, width, mode):
            with ZipFile(path, 'w') as z:
                z.writestr('xl/workbook.xml', '<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" r:id="sheet"/></sheets></workbook>')
                z.writestr('xl/_rels/workbook.xml.rels', '<Relationships><Relationship Id="sheet" Type="worksheet" Target="worksheets/sheet.xml"/></Relationships>')
                z.writestr('xl/worksheets/sheet.xml', f'<worksheet><cols><col min="1" max="1" width="{width}"/></cols></worksheet>')
                z.writestr('xl/worksheets/_rels/sheet.xml.rels', '<Relationships><Relationship Id="s" Type="slicer" Target="../slicer.xml"/><Relationship Id="d" Type="drawing" Target="../drawing.xml"/></Relationships>')
                z.writestr('xl/slicer.xml', '<slicers><slicer name="Status"/></slicers>')
                z.writestr('xl/drawing.xml', f'<wsDr><twoCellAnchor editAs="{mode}"><from><col>1</col><row>1</row></from><to><col>2</col><row>2</row></to><slicer name="Status"/></twoCellAnchor></wsDr>')

        script = Path(__file__).with_name('table_slicer_package_diff.py')
        with tempfile.TemporaryDirectory() as tmp:
            before, after = Path(tmp) / 'before.xlsx', Path(tmp) / 'after.xlsx'
            package(before, 8, 'twoCell')
            for width, mode, expected in [(8, 'twoCell', 0), (12, 'twoCell', 1), (8, 'absolute', 1)]:
                with self.subTest(width=width, mode=mode):
                    package(after, width, mode)
                    result = subprocess.run([sys.executable, str(script), str(before), str(after)], capture_output=True, text=True)
                    self.assertEqual(result.returncode, expected, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
