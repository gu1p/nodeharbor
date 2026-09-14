"""Portable contracts for the opaque worker backup stream; never use host worker paths."""
import importlib.util
import io
import os
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('storage_backup', Path(__file__).parents[1] / 'guest/storage_backup.py')
backup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(backup)


@unittest.skipIf(os.name == 'nt', 'The archive runs inside the Linux guest; native Multipass verification is required')
class WorkerBackup(unittest.TestCase):
    def test_whole_pool_preserves_dangling_symlinks_and_hardlinked_special_files(self):
        with tempfile.TemporaryDirectory() as source, tempfile.TemporaryDirectory() as target:
            source, target = Path(source), Path(target)
            (source / 'dangling').symlink_to('absent')
            os.mkfifo(source / 'pipe'); os.link(source / 'pipe', source / 'pipe-copy')
            stream = io.BytesIO(); backup.write_backup(source, stream, None)
            stream.seek(0); backup.restore_backup(target, stream, None)
            stream.seek(0); backup.verify_backup(target, stream, None)
            self.assertTrue((target / 'dangling').is_symlink())
            self.assertEqual((target / 'pipe').stat().st_ino, (target / 'pipe-copy').stat().st_ino)

    def test_filesystem_special_entries_preserve_named_pipes_without_reading_them(self):
        with tempfile.TemporaryDirectory() as source, tempfile.TemporaryDirectory() as target:
            source, target = Path(source), Path(target)
            (source / 'k3s').mkdir(); os.mkfifo(source / 'k3s/pipe', 0o640)
            stream = io.BytesIO(); backup.write_backup(source, stream)
            stream.seek(0); backup.restore_backup(target, stream)
            stream.seek(0); backup.verify_backup(target, stream)
            import stat
            self.assertTrue(stat.S_ISFIFO((target / 'k3s/pipe').stat().st_mode))
    def test_complete_pool_includes_files_outside_the_standard_kubernetes_directories(self):
        with tempfile.TemporaryDirectory() as source, tempfile.TemporaryDirectory() as target:
            source, target = Path(source), Path(target)
            (source / 'owner-data').write_bytes(b'preserve all pool contents')
            stream = io.BytesIO(); backup.write_backup(source, stream, None)
            stream.seek(0); backup.restore_backup(target, stream, None)
            stream.seek(0); backup.verify_backup(target, stream, None)
            self.assertEqual((target / 'owner-data').read_bytes(), b'preserve all pool contents')
    def fixture(self, root):
        data = root / 'k3s'; data.mkdir()
        (data / 'contents').write_bytes(b'worker data\x00\xff')
        (data / 'contents').chmod(0o640)
        os.link(data / 'contents', data / 'hardlink')
        (data / 'symlink').symlink_to('contents')
        with (data / 'sparse').open('wb') as handle:
            handle.seek(8 * 1024 * 1024); handle.write(b'end')
        if hasattr(os, 'setxattr'):
            os.setxattr(data / 'contents', 'user.nodeharbor', b'metadata\x00\xff')

    def test_stream_preserves_data_sparse_files_links_permissions_and_extended_attributes(self):
        with tempfile.TemporaryDirectory() as source, tempfile.TemporaryDirectory() as target:
            source, target = Path(source), Path(target); self.fixture(source)
            stream = io.BytesIO(); backup.write_backup(source, stream)
            self.assertLess(len(stream.getvalue()), 1024 * 1024)
            stream.seek(0); backup.verify_backup(source, stream)
            stream.seek(0); backup.restore_backup(target, stream)
            stream.seek(0); backup.verify_backup(target, stream)
            self.assertEqual((target / 'k3s/contents').stat().st_ino, (target / 'k3s/hardlink').stat().st_ino)
            self.assertEqual((target / 'k3s/contents').stat().st_mode & 0o777, 0o640)
            if (source / 'k3s/sparse').stat().st_blocks * 512 < 1024 * 1024:
                self.assertLess((target / 'k3s/sparse').stat().st_blocks * 512, 1024 * 1024)

    def test_changed_data_metadata_and_corrupt_or_interrupted_backups_fail_verification(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); self.fixture(root)
            stream = io.BytesIO(); backup.write_backup(root, stream); data = stream.getvalue()
            for invalid in (data[:-1], data[:30], data + b'extra'):
                with self.assertRaises((ValueError, EOFError)): backup.verify_backup(root, io.BytesIO(invalid))
            (root / 'k3s/contents').chmod(0o600)
            with self.assertRaises(ValueError): backup.verify_backup(root, io.BytesIO(data))

    def test_restore_refuses_symlinked_destination_and_path_traversal(self):
        with tempfile.TemporaryDirectory() as source, tempfile.TemporaryDirectory() as target:
            source, target = Path(source), Path(target); self.fixture(source)
            stream = io.BytesIO(); backup.write_backup(source, stream)
            (target / 'k3s').symlink_to(source / 'k3s', target_is_directory=True)
            stream.seek(0)
            with self.assertRaises(ValueError): backup.restore_backup(target, stream)

    def test_restore_resumes_after_interruption_including_already_restored_symbolic_links(self):
        with tempfile.TemporaryDirectory() as source, tempfile.TemporaryDirectory() as target:
            source, target = Path(source), Path(target); self.fixture(source)
            stream = io.BytesIO(); backup.write_backup(source, stream); data = stream.getvalue()
            with self.assertRaises(EOFError): backup.restore_backup(target, io.BytesIO(data[:-1]))
            backup.restore_backup(target, io.BytesIO(data))
            backup.verify_backup(target, io.BytesIO(data))
