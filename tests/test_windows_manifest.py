"""Desktop IPC tests need the supported Windows common-control activation context."""
from pathlib import Path
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]

class WindowsTestManifest(unittest.TestCase):
    def test_native_dialog_controls_load_without_requesting_elevation(self):
        manifest = ET.parse(ROOT / 'desktop/tests/windows.manifest').getroot()
        ns = {'assembly': 'urn:schemas-microsoft-com:asm.v1',
              'security': 'urn:schemas-microsoft-com:asm.v3'}
        dependency = manifest.find('assembly:dependency/assembly:dependentAssembly/assembly:assemblyIdentity', ns)
        self.assertEqual(dependency.attrib, {
            'type': 'win32', 'name': 'Microsoft.Windows.Common-Controls',
            'version': '6.0.0.0', 'processorArchitecture': '*',
            'publicKeyToken': '6595b64144ccf1df', 'language': '*'})
        privilege = manifest.find('security:trustInfo/security:security/security:requestedPrivileges/security:requestedExecutionLevel', ns)
        self.assertEqual(privilege.attrib, {'level': 'asInvoker', 'uiAccess': 'false'})
