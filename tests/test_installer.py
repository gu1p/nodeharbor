"""Exercise the shell installer with an isolated PATH and filesystem."""
import os
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]

@unittest.skipIf(os.name=='nt','The PowerShell installer is checked on Windows')
class InstallerBehavior(unittest.TestCase):
    def fixture(self, root, *, stalled=False, corrupt=False, copy_failure=False, malformed=None):
        tools=root/'tools';tools.mkdir();downloads=root/'downloads';downloads.mkdir()
        install=root/'Applications';old=install/'NodeHarbor.app';old_bin=old/'Contents'/'MacOS';old_bin.mkdir(parents=True)
        (old/'keep').write_text('previous application')
        def executable(path,body):
            path.write_text('#!/bin/sh\n'+body+'\n');path.chmod(0o755)
        executable(old_bin/'nodeharbor','echo "app $*" >> "$NODEHARBOR_INSTALL_TEST_LOG"')
        executable(old_bin/'nodeharbor-agent','echo "agent $*" >> "$NODEHARBOR_INSTALL_TEST_LOG"\n'+
                   ('if [ "$1" = wait-for-app-exit ]; then echo "The previous application is still running" >&2; exit 1; fi' if stalled else 'exit 0'))
        bundle=root/'bundle'/'NodeHarbor.app';(bundle/'Contents'/'MacOS').mkdir(parents=True)
        executable(bundle/'Contents'/'MacOS'/'nodeharbor','echo "NodeHarbor 0.1.12 ('+'a'*40+')"')
        package=downloads/'nodeharbor-v0.1.12-aarch64-apple-darwin.dmg';package.write_bytes(b'disk image fixture')
        asset={'name':package.name,'digest':'sha256:'+('0'*64 if corrupt else hashlib.sha256(package.read_bytes()).hexdigest()),
               'state':'uploaded','browser_download_url':'https://github.com/gu1p/nodeharbor/releases/download/v0.1.12/'+package.name}
        release={'tag_name':'v0.1.12','draft':False,'assets':[asset]}
        if malformed=='duplicate':release['assets'].append(asset.copy())
        elif malformed=='missing':release['assets']=[]
        elif malformed=='draft':release['draft']=True
        elif malformed=='version':release['tag_name']='v0.1.13'
        elif malformed=='digest':asset['digest']=None
        elif malformed=='url':asset['browser_download_url']='https://example.com/installer.dmg'
        elif malformed=='state':asset['state']='starter'
        (downloads/'release.json').write_text(json.dumps(release))
        (downloads/'mount.json').write_text(json.dumps({'system-entities':[
            {'dev-entry':'/dev/disk99','potentially-mountable':False},
            {'dev-entry':'/dev/disk99s1','mount-point':str(root/'bundle')}]}))
        executable(tools/'uname','if [ "$1" = -s ]; then echo Darwin; else echo arm64; fi')
        executable(tools/'sysctl','echo 1')
        executable(tools/'curl','''out=''; url=''
while [ "$#" -gt 0 ]; do
 case "$1" in -o) shift; out=$1;; https:*) url=$1;; esac
 shift
done
echo "download $url" >> "$NODEHARBOR_INSTALL_TEST_DOWNLOAD_LOG"
case "$url" in
 https://api.github.com/repos/gu1p/nodeharbor/releases/tags/v0.1.12) cp "$NODEHARBOR_INSTALL_TEST_DOWNLOADS/release.json" "$out";;
 https://github.com/gu1p/nodeharbor/releases/download/v0.1.12/*.dmg) cp "$NODEHARBOR_INSTALL_TEST_DOWNLOADS/${url##*/}" "$out";;
 *) echo "Unexpected download: $url" >&2; exit 1;;
esac''')
        executable(tools/'hdiutil','''echo "disk $*" >> "$NODEHARBOR_INSTALL_TEST_LOG"
if [ "$1" = attach ]; then
 for arg in "$@"; do
  if [ "$arg" = -mountpoint ]; then echo 'Custom external-drive mount point denied' >&2; exit 13; fi
 done
 cat "$NODEHARBOR_INSTALL_TEST_DOWNLOADS/mount.json"
fi''')
        executable(tools/'ditto','echo "copy $*" >> "$NODEHARBOR_INSTALL_TEST_LOG"\n'+('exit 42' if copy_failure else 'cp -R "$1" "$2"'))
        env={**os.environ,'PATH':str(tools)+os.pathsep+os.environ['PATH'],'NODEHARBOR_VERSION':'0.1.12',
             'NODEHARBOR_INSTALL_DIR':str(install),'NODEHARBOR_INSTALL_TEST_DOWNLOADS':str(downloads),
            'NODEHARBOR_INSTALL_TEST_LOG':str(root/'calls'),'NODEHARBOR_INSTALL_TEST_DOWNLOAD_LOG':str(root/'downloads.log')}
        return install, env

    def test_installation_uses_a_verified_disk_image_without_auxiliary_release_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);install,env=self.fixture(root)
            result=subprocess.run(['bash',str(ROOT/'get-nodeharbor.sh')],env=env,capture_output=True,text=True)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertTrue((install/'NodeHarbor.app/Contents/MacOS/nodeharbor').is_file())
            self.assertFalse((install/'NodeHarbor.app/keep').exists())
            self.assertEqual(len(list(install.glob('.NodeHarbor.previous.*.app'))),1)
            calls=(root/'calls').read_text()
            self.assertIn('-readonly',calls);self.assertIn('disk detach ',calls)
            self.assertNotIn('-mountpoint',calls)
            self.assertEqual(len((root/'downloads.log').read_text().splitlines()),2)
            self.assertFalse(list(install.glob('.nodeharbor-stage.*')))

    def test_invalid_release_metadata_never_mounts_or_changes_the_existing_app(self):
        for malformed in ['duplicate','missing','draft','version','digest','url','state']:
            with self.subTest(malformed=malformed), tempfile.TemporaryDirectory() as directory:
                root=Path(directory);install,env=self.fixture(root,malformed=malformed)
                result=subprocess.run(['bash',str(ROOT/'get-nodeharbor.sh')],env=env,capture_output=True,text=True)
                self.assertNotEqual(result.returncode,0)
                self.assertEqual((install/'NodeHarbor.app/keep').read_text(),'previous application')
                self.assertFalse((root/'calls').exists())

    def test_copy_failure_detaches_the_disk_image_and_preserves_the_existing_app(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);install,env=self.fixture(root,copy_failure=True)
            result=subprocess.run(['bash',str(ROOT/'get-nodeharbor.sh')],env=env,capture_output=True,text=True)
            self.assertNotEqual(result.returncode,0)
            self.assertIn('disk detach ',(root/'calls').read_text())
            self.assertEqual((install/'NodeHarbor.app/keep').read_text(),'previous application')
            self.assertFalse(list(install.glob('.nodeharbor-stage.*')))

    def test_an_update_preserves_the_old_app_and_removes_staging_when_quit_is_not_finished(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);install,env=self.fixture(root,stalled=True)
            result=subprocess.run(['bash',str(ROOT/'get-nodeharbor.sh')],env=env,capture_output=True,text=True)
            self.assertNotEqual(result.returncode,0,'An update cannot replace a still-running application')
            self.assertIn('still running',result.stderr)
            self.assertEqual((install/'NodeHarbor.app/keep').read_text(),'previous application')
            calls=[line for line in (root/'calls').read_text().splitlines() if line.startswith(('app ','agent '))]
            self.assertEqual(calls[:2],['agent prepare-update','app --quit'])
            self.assertTrue(calls[2].startswith('agent wait-for-app-exit --executable '))
            self.assertFalse(list(install.glob('.nodeharbor-stage.*')))

    def test_native_apple_silicon_is_selected_even_under_rosetta(self):
        with tempfile.TemporaryDirectory() as directory:
            folder=Path(directory)
            for name,body in {'uname':'if [ "$1" = -s ]; then echo Darwin; else echo x86_64; fi','sysctl':'echo 1'}.items():
                path=folder/name;path.write_text('#!/bin/sh\n'+body+'\n');path.chmod(0o755)
            result=subprocess.run(['bash','-c','source "$1"; nodeharbor_target','test',str(ROOT/'get-nodeharbor.sh')],env={**os.environ,'PATH':str(folder)+os.pathsep+os.environ['PATH']},capture_output=True,text=True)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(result.stdout.strip(),'aarch64-apple-darwin')

    def test_a_corrupt_download_preserves_the_previous_installation_and_settings(self):
        with tempfile.TemporaryDirectory() as directory:
            folder=Path(directory);install,env=self.fixture(folder,corrupt=True)
            original=install/'NodeHarbor.app/keep'
            settings=folder/'settings';settings.write_text('private-device-state')
            result=subprocess.run(['bash',str(ROOT/'get-nodeharbor.sh')],env=env,capture_output=True,text=True)
            self.assertNotEqual(result.returncode,0)
            self.assertIn('checksum',result.stderr.lower())
            self.assertEqual(original.read_text(),'previous application')
            self.assertEqual(settings.read_text(),'private-device-state')

    def test_an_unsupported_platform_is_rejected_before_network_or_installation_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            folder=Path(directory);uname=folder/'uname';uname.write_text('#!/bin/sh\necho FreeBSD\n');uname.chmod(0o755)
            result=subprocess.run(['bash','-c','source "$1"; nodeharbor_target','test',str(ROOT/'get-nodeharbor.sh')],env={**os.environ,'PATH':str(folder)+os.pathsep+os.environ['PATH']},capture_output=True,text=True)
            self.assertNotEqual(result.returncode,0)
            self.assertIn('unsupported',result.stderr.lower())

@unittest.skipUnless(__import__('sys').platform.startswith('linux'), 'Linux activation uses native GNU filesystem commands')
class LinuxActivation(unittest.TestCase):
    def activate(self, root, *, existing=True, fail=False):
        install=root/'app';install.mkdir();bin_dir=root/'bin';bin_dir.mkdir();menu=root/'menu';menu.mkdir()
        old=install/'old';old.mkdir();new=install/'new';new.mkdir();(new/'AppRun').write_text('new application')
        launcher=bin_dir/'nodeharbor';entry=menu/'nodeharbor.desktop'
        if existing:
            (install/'current').symlink_to(old,target_is_directory=True)
            launcher.symlink_to(install/'current'/'AppRun');entry.write_text('previous desktop entry')
        tools=root/'tools';tools.mkdir()
        if fail:
            mv=tools/'mv';mv.write_text('#!/bin/bash\nif [[ "${*: -1}" == "$NODEHARBOR_TEST_FAIL_DEST" ]]; then exit 42; fi\nexec /usr/bin/mv "$@"\n');mv.chmod(0o755)
        result=subprocess.run(['bash','-c','source "$1"; nodeharbor_activate_linux "$2" "$3" "$4" "$5"','test',str(ROOT/'get-nodeharbor.sh'),str(install),str(new),str(launcher),str(entry)],env={**os.environ,'PATH':str(tools)+os.pathsep+os.environ['PATH'],'NODEHARBOR_TEST_FAIL_DEST':str(install/'current')},capture_output=True,text=True)
        return result,install,old,new,launcher,entry

    def test_update_failure_restores_launchers_and_preserves_the_active_application(self):
        with tempfile.TemporaryDirectory() as directory:
            result,install,old,new,launcher,entry=self.activate(Path(directory),fail=True)
            self.assertEqual(result.returncode,42,result.stderr)
            self.assertEqual((install/'current').resolve(),old)
            self.assertTrue(launcher.is_symlink())
            self.assertEqual(os.readlink(launcher),str(install/'current'/'AppRun'))
            self.assertEqual(entry.read_text(),'previous desktop entry')
            self.assertFalse(list(Path(directory).rglob('.nodeharbor-activation.*')))

    def test_first_install_failure_leaves_no_broken_application_menu_entry(self):
        with tempfile.TemporaryDirectory() as directory:
            result,install,old,new,launcher,entry=self.activate(Path(directory),existing=False,fail=True)
            self.assertEqual(result.returncode,42,result.stderr)
            self.assertFalse((install/'current').is_symlink())
            self.assertFalse(launcher.is_symlink());self.assertFalse(entry.exists())

    def test_success_publishes_the_version_after_installing_its_launchers(self):
        with tempfile.TemporaryDirectory() as directory:
            result,install,old,new,launcher,entry=self.activate(Path(directory))
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual((install/'current').resolve(),new)
            self.assertEqual(launcher.resolve(),new/'AppRun')
            self.assertIn('Name=NodeHarbor',entry.read_text())
            self.assertTrue(old.is_dir())
            self.assertFalse(list(Path(directory).rglob('.nodeharbor-activation.*')))
