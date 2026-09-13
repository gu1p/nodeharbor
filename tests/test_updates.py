import sys
import base64
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import test_release as release_tests
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'scripts'))
spec=importlib.util.spec_from_file_location('updates',ROOT/'scripts/updates.py')
updates=importlib.util.module_from_spec(spec);spec.loader.exec_module(updates)
class UpdateChannel(unittest.TestCase):
 def test_every_installer_format_gets_a_signed_target_without_public_release_clutter(self):
  with tempfile.TemporaryDirectory() as directory:
   folder=Path(directory)/'dist';folder.mkdir();release_tests.ReleaseContract().fixture(folder)
   def signing(path,key):return base64.b64encode(('signed '+path.name).encode()).decode()
   output=Path(directory)/'site'
   with patch.object(updates,'sign',side_effect=signing):
    updates.build(folder,output,'0.1.12','a'*40,Path(directory)/'key')
   manifest=json.loads((output/'updates/latest.json').read_text())
   self.assertEqual(manifest['version'],'0.1.12')
   self.assertEqual(set(manifest['platforms']),{'darwin-aarch64-app','darwin-x86_64-app','linux-aarch64-deb','linux-x86_64-deb','linux-aarch64-appimage','linux-x86_64-appimage','windows-x86_64-nsis'})
   for platform,asset in manifest['platforms'].items():
    self.assertTrue(asset['url'].startswith('https://'))
    self.assertIn('signed ',base64.b64decode(asset['signature']).decode())
    if platform.startswith('darwin'):
     self.assertIn('gu1p.github.io/nodeharbor/updates/v0.1.12/',asset['url'])
     self.assertTrue((output/'updates/v0.1.12'/asset['url'].split('/')[-1]).is_file())
    else:self.assertIn('github.com/gu1p/nodeharbor/releases/download/v0.1.12/',asset['url'])
   self.assertEqual(len(list(output.rglob('*.tar.gz'))),2)
   self.assertFalse(list(output.rglob('*.minisig')))
 def test_incomplete_or_tampered_packages_never_enter_the_update_channel(self):
  with tempfile.TemporaryDirectory() as directory:
   folder=Path(directory)/'dist';folder.mkdir();release_tests.ReleaseContract().fixture(folder)
   next(folder.glob('*.exe')).write_bytes(b'tampered')
   with patch.object(updates,'sign') as sign:
    with self.assertRaises(ValueError):updates.build(folder,Path(directory)/'site','0.1.12','a'*40,Path(directory)/'key')
    sign.assert_not_called()
 def test_older_finishing_build_cannot_downgrade_the_channel(self):
  self.assertTrue(updates.should_publish('0.1.12',None))
  self.assertTrue(updates.should_publish('0.1.12',{'version':'0.1.11'}))
  self.assertFalse(updates.should_publish('0.1.12',{'version':'0.1.12'}))
  self.assertFalse(updates.should_publish('0.1.12',{'version':'0.1.13'}))
  with self.assertRaises(ValueError):updates.should_publish('0.1.12',{'error':'broken manifest'})
 @unittest.skipUnless(shutil.which('minisign'),'native signing tool is required')
 def test_real_signatures_authenticate_payload_bytes(self):
  with tempfile.TemporaryDirectory() as directory:
   folder=Path(directory);key=folder/'key';public=folder/'key.pub';payload=folder/'payload';payload.write_bytes(b'update payload')
   subprocess.run(['minisign','-G','-W','-s',str(key),'-p',str(public)],check=True,capture_output=True)
   with patch.object(updates,'PUBLIC_KEY',public):signature=updates.sign(payload,key)
   sig=folder/'payload.sig';sig.write_bytes(base64.b64decode(signature))
   result=subprocess.run(['minisign','-V','-p',str(public),'-m',str(payload),'-x',str(sig)],capture_output=True)
   self.assertEqual(result.returncode,0)
   payload.write_bytes(b'tampered')
   self.assertNotEqual(subprocess.run(['minisign','-V','-p',str(public),'-m',str(payload),'-x',str(sig)],capture_output=True).returncode,0)

class UpdaterWiring(unittest.TestCase):
 def test_signed_channel_is_deployed_only_after_the_complete_release(self):
  workflow=(ROOT/'.github/workflows/release.yml').read_text()
  channel=workflow.split('  updates:')[1]
  self.assertIn('needs: [identity, publish]',channel)
  self.assertIn('pages: write',channel)
  self.assertIn('id-token: write',channel)
  self.assertIn('scripts/updates.py',channel)
  self.assertIn('actions/deploy-pages@',channel)
  self.assertIn('steps.channel.outputs.deploy',channel)
  self.assertNotIn('gh release upload',channel)
 def test_trusted_key_and_https_endpoint_are_embedded_in_the_desktop(self):
  config=json.loads((ROOT/'desktop/tauri.conf.json').read_text())['plugins']['updater']
  self.assertEqual(base64.b64decode(config['pubkey']).decode().splitlines(),(ROOT/'nodeharbor.minisign.pub').read_text().splitlines())
  self.assertEqual(config['endpoints'],['https://gu1p.github.io/nodeharbor/updates/latest.json'])
  self.assertFalse(any(value for key,value in config.items() if key.startswith('dangerous')))
