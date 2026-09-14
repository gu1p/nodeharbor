import importlib.util
from pathlib import Path
import tempfile
import unittest
import json
import shutil
import tomllib
import io
from contextlib import redirect_stdout
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("release", ROOT / "scripts" / "release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)

class ReleaseContract(unittest.TestCase):
    def remote_installers(self, folder):
        return [{'name':path.name,'digest':'sha256:'+release.checksum(path),'state':'uploaded'}
                for path in sorted(folder.iterdir()) if path.name.endswith(('.exe','.dmg','.deb','.AppImage','.apk','-runtime-source.tar.gz'))]

    def test_download_assets_contain_only_installers_and_build_evidence_stays_internal(self):
        with tempfile.TemporaryDirectory() as directory:
            folder=Path(directory);self.fixture(folder)
            draft={'draft':True,'target_commitish':'a'*40,'assets':self.remote_installers(folder)}
            commands=[]
            with patch.object(release,'github_release',side_effect=[None,draft]), patch.object(release.subprocess,'run',side_effect=lambda args,**kwargs:commands.append(args)):
                release.publish(folder,'0.1.12','a'*40,'owner/project',folder/'key')
            upload=next(command for command in commands if command[:3]==['gh','release','upload'])
            self.assertEqual({Path(path).name for path in upload[7:]},{asset['name'] for asset in draft['assets']})
            self.assertEqual(len(draft['assets']),9)
            self.assertTrue((folder/'release-manifest.json').is_file())
            self.assertTrue((folder/'SHA256SUMS').is_file())
            notes=(folder/'release-notes.md').read_text()
            self.assertIn('Windows x64',notes)
            self.assertIn('Android ARM64',notes)
            self.assertIn('runtime-source.tar.gz',notes)
            self.assertNotIn('`SHA256SUMS`',notes)

    def test_publication_can_be_retried_after_removing_auxiliary_downloads(self):
        with tempfile.TemporaryDirectory() as directory:
            folder=Path(directory);self.fixture(folder)
            published={'draft':False,'target_commitish':'a'*40,'assets':self.remote_installers(folder)}
            with patch.object(release,'github_release',return_value=published), patch.object(release.subprocess,'run') as command:
                release.publish(folder,'0.1.12','a'*40,'owner/project',folder/'key')
            command.assert_not_called()

    def test_a_draft_with_an_incomplete_or_changed_uploaded_installer_cannot_be_published(self):
        with tempfile.TemporaryDirectory() as directory:
            folder=Path(directory);self.fixture(folder)
            for change in ['missing','digest','state']:
                assets=self.remote_installers(folder)
                if change=='missing':assets.pop()
                elif change=='digest':assets[0]['digest']='sha256:'+'0'*64
                else:assets[0]['state']='starter'
                draft={'draft':True,'target_commitish':'a'*40,'assets':assets}
                with self.subTest(change=change), patch.object(release,'github_release',side_effect=[None,draft]), patch.object(release.subprocess,'run') as command:
                    with self.assertRaises(ValueError):release.publish(folder,'0.1.12','a'*40,'owner/project',folder/'key')
                    self.assertFalse(any(call.args[0][:3]==['gh','release','edit'] for call in command.call_args_list))

    def test_complete_draft_can_be_found_and_published_before_its_git_tag_exists(self):
        draft={'id':12,'tag_name':'v0.1.12','draft':True,'target_commitish':'a'*40,'assets':[]}
        requests=[];commands=[]
        def response(request, timeout):
            requests.append(request.full_url)
            if '/releases/tags/' in request.full_url:
                raise release.urllib.error.HTTPError(request.full_url,404,'Not Found',{},None)
            return io.BytesIO(json.dumps([draft]).encode())
        with tempfile.TemporaryDirectory() as directory:
            folder=Path(directory);self.fixture(folder)
            draft['assets']=self.remote_installers(folder)
            with patch.dict(release.os.environ,{'GH_TOKEN':'test-token'}), patch.object(release.urllib.request,'urlopen',side_effect=response), patch.object(release.subprocess,'run',side_effect=lambda args,**kwargs:commands.append(args)):
                release.publish(folder,'0.1.12','a'*40,'owner/project',folder/'key')
        self.assertFalse(any(command[:3]==['gh','release','create'] for command in commands))
        self.assertIn(['gh','release','edit','v0.1.12','--repo','owner/project','--draft=false','--latest=false'],commands)
        self.assertTrue(all('/releases?' in url for url in requests))

    def test_draft_lookup_pages_past_newer_releases(self):
        unrelated=[{'tag_name':f'v1.0.{index}'} for index in range(100)]
        wanted={'id':12,'tag_name':'v0.1.12','draft':True,'target_commitish':'a'*40}
        with patch.dict(release.os.environ,{'GH_TOKEN':'test-token'}), patch.object(release.urllib.request,'urlopen',side_effect=[io.BytesIO(json.dumps(unrelated).encode()),io.BytesIO(json.dumps([wanted]).encode())]) as request:
            self.assertEqual(release.github_release('owner/project','v0.1.12'),wanted)
        self.assertIn('page=2',request.call_args.args[0].full_url)

    def test_missing_release_is_distinct_from_missing_repository_access(self):
        error=release.urllib.error.HTTPError('https://api.github.com',404,'Not Found',{},None)
        with patch.dict(release.os.environ,{'GH_TOKEN':'test-token'}), patch.object(release.urllib.request,'urlopen',return_value=io.BytesIO(b'[]')):
            self.assertIsNone(release.github_release('owner/project','v0.1.12'))
        with patch.dict(release.os.environ,{'GH_TOKEN':'test-token'}), patch.object(release.urllib.request,'urlopen',side_effect=error):
            with self.assertRaises(release.urllib.error.HTTPError):
                release.github_release('owner/project','v0.1.12')
        error.close()

    def fixture(self, folder):
        for target, (os_name, arch, extensions) in release.TARGETS.items():
            assets=[]
            for extension in extensions:
                name=f'nodeharbor-v0.1.12-{target}{extension}'
                (folder/name).write_bytes(b'native-package-fixture')
                assets.append({'name':name,'sha256':release.checksum(folder/name)})
            manifest = {'version':'0.1.12','commit':'a'*40,'target':target,'os':os_name,'arch':arch,'assets':assets}
            if os_name == 'android':
                manifest['qualification'] = dict(version='0.1.12', commit='a'*40, apkSha256=assets[0]['sha256'],
                    certificateSha256='c'*64, signedRelease=True, physical=True, apiLevels=[33,36,37],
                    ownerControlsPassed=True, vpnPreserved=True, ciQualified=True, arm64JobSucceeded=True)
            (folder/f'nodeharbor-v0.1.12-{target}.json').write_text(json.dumps(manifest))

    def test_complete_packages_are_accepted_but_tampering_and_wrong_architecture_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            folder=Path(directory); self.fixture(folder)
            self.assertEqual(len(release.validate_assets(folder,'0.1.12','a'*40)),6)
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

    def test_a_published_release_is_immutable_and_rerunning_the_same_release_is_safe(self):
        asset={'name':'nodeharbor-v0.1.12-x86_64-pc-windows-msvc.exe','sha256':'c'*64}
        manifest={'version':'0.1.12','commit':'a'*40,'targets':[{'assets':[asset]}]}
        remote={'name':asset['name'],'digest':'sha256:'+asset['sha256'],'state':'uploaded'}
        self.assertEqual(release.publication_action(None,manifest),'create')
        self.assertEqual(release.publication_action({'draft':True,'target_commitish':'a'*40},manifest),'resume')
        self.assertEqual(release.publication_action({'draft':False,'target_commitish':'a'*40,'assets':[remote]},manifest),'skip')
        with self.assertRaises(ValueError):release.publication_action({'draft':False,'target_commitish':'b'*40,'assets':[remote]},manifest)
        for assets in [[],[dict(remote,digest='sha256:'+'d'*64)],[remote,remote]]:
            with self.assertRaises(ValueError):release.publication_action({'draft':False,'target_commitish':'a'*40,'assets':assets},manifest)
    def test_versions_are_stable_and_ordered_by_main_history(self):
        self.assertEqual(release.version_for("0.1.0", 12), "0.1.12")
        self.assertEqual(release.version_for("0.1.0", 12), release.version_for("0.1.0", 12))
        for invalid in [0, -1]:
            with self.assertRaises(ValueError): release.version_for("0.1.0", invalid)

    def test_rebased_remote_configuration_releases_upgrade_the_published_0_1_line(self):
        versions=[]
        for position in [65,65,66]:
            output=io.StringIO()
            with patch('sys.argv',['release.py','version']), patch.object(release.subprocess,'check_output',return_value=str(position)), redirect_stdout(output):
                release.main()
            versions.append(tuple(map(int,output.getvalue().strip().split('.'))))
        self.assertGreater(versions[0],(0,1,72))
        self.assertEqual(versions[0],versions[1])
        self.assertGreater(versions[2],versions[1])

    def test_only_a_complete_matching_release_can_be_published(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory)
            with self.assertRaises(ValueError): release.validate_assets(folder, "0.1.12", "a" * 40)

    def test_all_five_desktop_targets_and_android_have_unique_installable_assets(self):
        targets = release.TARGETS
        self.assertEqual(len(targets), 6)
        self.assertEqual(len(set(targets)), 6)
        self.assertEqual(targets['aarch64-linux-android'], ('android', 'arm64', ['.apk', '-runtime-source.tar.gz']))
        self.assertIn("x86_64-pc-windows-msvc", targets)
        self.assertIn("aarch64-apple-darwin", targets)
        self.assertIn("aarch64-unknown-linux-gnu", targets)

    def test_linux_tools_include_the_controller_probe_and_its_dashboard(self):
        import tarfile
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);target='aarch64-unknown-linux-gnu';build=root/'target'/target/'release'
            for name in ['bundle/deb/app.deb','bundle/appimage/app.AppImage','nodeharbor-agent','nodeharbor-controller','nodeharbor-probe']:
                path=build/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(b'tested-build')
            (root/'ui/dist').mkdir(parents=True);(root/'ui/dist/index.html').write_text('dashboard')
            release.collect(root,root/'target',root/'dist',target,'0.1.12','a'*40)
            archive=next((root/'dist').glob('*-tools.tar.gz'))
            with tarfile.open(archive) as package:
                self.assertTrue({'nodeharbor-agent','nodeharbor-controller','nodeharbor-probe','ui/index.html'}.issubset(package.getnames()))

    def test_image_publication_waits_for_all_platforms_and_is_required_before_a_release(self):
        workflow=(ROOT/'.github/workflows/release.yml').read_text()
        self.assertIn('container-check:\n    needs: [identity, native, android]',workflow)
        self.assertIn('needs: [identity, native, container-check]',workflow)
        self.assertIn('needs: [identity, native, container-publish, android-qualification]',workflow)
