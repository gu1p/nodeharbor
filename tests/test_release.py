import importlib.util
from pathlib import Path
import tempfile
import unittest
import json
import shutil
import tomllib

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("release", ROOT / "scripts" / "release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)

class ReleaseContract(unittest.TestCase):
    def fixture(self, folder):
        for target, (os_name, arch, extensions) in release.TARGETS.items():
            assets=[]
            for extension in extensions:
                name=f'nodeharbor-v0.1.12-{target}{extension}'
                (folder/name).write_bytes(b'native-package-fixture')
                assets.append({'name':name,'sha256':release.checksum(folder/name)})
            (folder/f'nodeharbor-v0.1.12-{target}.json').write_text(json.dumps({'version':'0.1.12','commit':'a'*40,'target':target,'os':os_name,'arch':arch,'assets':assets}))

    def test_complete_packages_are_accepted_but_tampering_and_wrong_architecture_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            folder=Path(directory); self.fixture(folder)
            self.assertEqual(len(release.validate_assets(folder,'0.1.12','a'*40)),5)
            path=folder/'nodeharbor-v0.1.12-aarch64-apple-darwin.json'
            data=json.loads(path.read_text());data['arch']='amd64';path.write_text(json.dumps(data))
            with self.assertRaises(ValueError):release.validate_assets(folder,'0.1.12','a'*40)
            self.fixture(folder)
            next(folder.glob('*.exe')).write_bytes(b'tampered')
            with self.assertRaises(ValueError):release.validate_assets(folder,'0.1.12','a'*40)

    def test_one_asset_cannot_claim_to_be_packages_for_two_platforms(self):
        with tempfile.TemporaryDirectory() as directory:
            folder=Path(directory);self.fixture(folder)
            linux=folder/'nodeharbor-v0.1.12-aarch64-unknown-linux-gnu.json'
            intel=json.loads((folder/'nodeharbor-v0.1.12-x86_64-unknown-linux-gnu.json').read_text())
            data=json.loads(linux.read_text());data['assets']=intel['assets'];linux.write_text(json.dumps(data))
            with self.assertRaises(ValueError):release.validate_assets(folder,'0.1.12','a'*40)

    def test_release_stamping_keeps_all_native_and_ui_versions_in_sync(self):
        with tempfile.TemporaryDirectory() as directory:
            folder=Path(directory)
            for name in ['Cargo.toml','Cargo.lock','desktop/tauri.conf.json','ui/package.json','ui/package-lock.json']:
                (folder/name).parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(ROOT/name,folder/name)
            release.stamp(folder,'0.1.12','a'*40)
            self.assertEqual(tomllib.loads((folder/'Cargo.toml').read_text())['workspace']['package']['version'],'0.1.12')
            lock=tomllib.loads((folder/'Cargo.lock').read_text())
            self.assertTrue(all(p['version']=='0.1.12' for p in lock['package'] if p['name'].startswith('nodeharbor-')))
            self.assertEqual(json.loads((folder/'desktop/tauri.conf.json').read_text())['version'],'0.1.12')
            self.assertEqual(json.loads((folder/'ui/package.json').read_text())['version'],'0.1.12')

    def test_late_builds_cannot_move_latest_to_an_older_main_commit(self):
        releases=[{'tag_name':'v0.1.8','target_commitish':'old','draft':False,'prerelease':False},
                  {'tag_name':'v0.1.9','target_commitish':'new','draft':False,'prerelease':False},
                  {'tag_name':'v0.1.10','target_commitish':'pending','draft':True,'prerelease':False}]
        self.assertEqual(release.latest_release(releases,['pending','new','old']),'v0.1.9')
        self.assertEqual(release.latest_release(list(reversed(releases)),['pending','new','old']),'v0.1.9')
    def test_versions_are_stable_and_ordered_by_main_history(self):
        self.assertEqual(release.version_for("0.1.0", 12), "0.1.12")
        self.assertEqual(release.version_for("0.1.0", 12), release.version_for("0.1.0", 12))
        for invalid in [0, -1]:
            with self.assertRaises(ValueError): release.version_for("0.1.0", invalid)

    def test_only_a_complete_matching_release_can_be_published(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory)
            with self.assertRaises(ValueError): release.validate_assets(folder, "0.1.12", "a" * 40)

    def test_all_five_native_targets_have_unique_installable_assets(self):
        targets = release.TARGETS
        self.assertEqual(len(targets), 5)
        self.assertEqual(len(set(targets)), 5)
        self.assertIn("x86_64-pc-windows-msvc", targets)
        self.assertIn("aarch64-apple-darwin", targets)
        self.assertIn("aarch64-unknown-linux-gnu", targets)
