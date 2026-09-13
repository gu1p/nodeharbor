"""Exercise the shell installer with an isolated PATH and filesystem."""
import os
import hashlib
from pathlib import Path
import subprocess
import tempfile
import tarfile
import unittest

ROOT=Path(__file__).resolve().parents[1]

@unittest.skipIf(os.name=='nt','The PowerShell installer is checked on Windows')
class InstallerBehavior(unittest.TestCase):
    def test_an_update_preserves_the_old_app_and_removes_staging_when_quit_is_not_finished(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);tools=root/'tools';tools.mkdir();downloads=root/'downloads';downloads.mkdir()
            install=root/'Applications';old=install/'NodeHarbor.app';old_bin=old/'Contents'/'MacOS';old_bin.mkdir(parents=True)
            (old/'keep').write_text('previous application')
            def executable(path,body):
                path.write_text('#!/bin/sh\n'+body+'\n');path.chmod(0o755)
            executable(old_bin/'nodeharbor','echo "app $*" >> "$NODEHARBOR_INSTALL_TEST_LOG"')
            executable(old_bin/'nodeharbor-agent','''echo "agent $*" >> "$NODEHARBOR_INSTALL_TEST_LOG"
if [ "$1" = wait-for-app-exit ]; then echo 'The previous application is still running' >&2; exit 1; fi''')
            bundle=root/'bundle'/'NodeHarbor.app';(bundle/'Contents'/'MacOS').mkdir(parents=True)
            executable(bundle/'Contents'/'MacOS'/'nodeharbor','echo "NodeHarbor 0.1.12 test-source"')
            package=downloads/'nodeharbor-v0.1.12-aarch64-apple-darwin.app.tar.gz'
            with tarfile.open(package,'w:gz') as archive:archive.add(bundle,arcname='NodeHarbor.app')
            (downloads/'SHA256SUMS').write_text(hashlib.sha256(package.read_bytes()).hexdigest()+'  '+package.name+'\n')
            executable(tools/'uname','if [ "$1" = -s ]; then echo Darwin; else echo arm64; fi')
            executable(tools/'sysctl','echo 1')
            executable(tools/'curl','''out=''
while [ "$#" -gt 0 ]; do
 if [ "$1" = -o ]; then shift; out=$1; fi
 shift
done
cp "$NODEHARBOR_INSTALL_TEST_DOWNLOADS/${out##*/}" "$out"''')
            log=root/'calls'
            result=subprocess.run(['bash',str(ROOT/'get-nodeharbor.sh')],env={**os.environ,'PATH':str(tools)+os.pathsep+os.environ['PATH'],
                'NODEHARBOR_VERSION':'0.1.12','NODEHARBOR_INSTALL_DIR':str(install),
                'NODEHARBOR_INSTALL_TEST_DOWNLOADS':str(downloads),'NODEHARBOR_INSTALL_TEST_LOG':str(log)},capture_output=True,text=True)
            self.assertNotEqual(result.returncode,0,'An update cannot replace a still-running application')
            self.assertIn('still running',result.stderr)
            self.assertEqual((old/'keep').read_text(),'previous application')
            calls=log.read_text().splitlines()
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
            folder=Path(directory);bin_dir=folder/'bin';bin_dir.mkdir()
            install=folder/'Applications';app=install/'NodeHarbor.app';app.mkdir(parents=True)
            original=app/'keep';original.write_text('previous-installation')
            settings=folder/'settings';settings.write_text('private-device-state')
            tools={'uname':'if [ "$1" = -s ]; then echo Darwin; else echo arm64; fi','sysctl':'echo 1',
                'curl':"""out=''
while [ "$#" -gt 0 ]; do
 if [ "$1" = -o ]; then shift; out=$1; fi
 shift
done
case "$out" in
 *SHA256SUMS) echo '0000000000000000000000000000000000000000000000000000000000000000  nodeharbor-v0.1.12-aarch64-apple-darwin.app.tar.gz' > "$out";;
 *) echo 'corrupt-package' > "$out";;
esac"""}
            for name,body in tools.items():
                path=bin_dir/name;path.write_text('#!/bin/sh\n'+body+'\n');path.chmod(0o755)
            result=subprocess.run(['bash',str(ROOT/'get-nodeharbor.sh')],env={**os.environ,'PATH':str(bin_dir)+os.pathsep+os.environ['PATH'],'NODEHARBOR_VERSION':'0.1.12','NODEHARBOR_INSTALL_DIR':str(install)},capture_output=True,text=True)
            self.assertNotEqual(result.returncode,0)
            self.assertIn('checksum',result.stderr.lower())
            self.assertEqual(original.read_text(),'previous-installation')
            self.assertEqual(settings.read_text(),'private-device-state')

    def test_an_unsupported_platform_is_rejected_before_network_or_installation_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            folder=Path(directory);uname=folder/'uname';uname.write_text('#!/bin/sh\necho FreeBSD\n');uname.chmod(0o755)
            result=subprocess.run(['bash','-c','source "$1"; nodeharbor_target','test',str(ROOT/'get-nodeharbor.sh')],env={**os.environ,'PATH':str(folder)+os.pathsep+os.environ['PATH']},capture_output=True,text=True)
            self.assertNotEqual(result.returncode,0)
            self.assertIn('unsupported',result.stderr.lower())
