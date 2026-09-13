import importlib.util
from pathlib import Path
import hashlib
import io
import tarfile
import tempfile
import unittest
import struct

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('android_runtime', ROOT / 'scripts/android_runtime.py')
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


class AndroidRuntimeContract(unittest.TestCase):
    def test_apk_runtime_requires_arm64_shared_libraries_with_16k_page_alignment(self):
        header = bytearray(64 + 56)
        header[:7] = b'\x7fELF\x02\x01\x01'
        struct.pack_into('<HHI', header, 16, 3, 183, 1)
        struct.pack_into('<Q', header, 32, 64)
        struct.pack_into('<HHH', header, 52, 64, 56, 1)
        struct.pack_into('<IIQQQQQQ', header, 64, 1, 5, 0, 0, 0, len(header), len(header), 16384)
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'runtime.so'
            path.write_bytes(header)
            runtime.verify_android_elf(path)
            for offset, value in [(18, 62), (112, 4096)]:
                broken = header.copy()
                struct.pack_into('<H' if offset == 18 else '<Q', broken, offset, value)
                path.write_bytes(broken)
                with self.subTest(offset=offset), self.assertRaises(ValueError): runtime.verify_android_elf(path)

    def test_download_is_only_accepted_after_its_complete_digest_matches(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'guest.img'
            path.write_bytes(b'complete guest')
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            runtime.verify_file(path, digest)
            path.write_bytes(b'truncated guest')
            with self.assertRaises(ValueError):
                runtime.verify_file(path, digest)

    def test_extraction_rejects_paths_and_links_that_escape_the_download(self):
        for name, link in [('../escape', None), ('/absolute', None), ('guest/link', '../../escape')]:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as folder:
                archive = Path(folder) / 'source.tar'
                with tarfile.open(archive, 'w') as output:
                    entry = tarfile.TarInfo(name)
                    if link:
                        entry.type = tarfile.SYMTYPE
                        entry.linkname = link
                        output.addfile(entry)
                    else:
                        entry.size = 4
                        output.addfile(entry, io.BytesIO(b'data'))
                with self.assertRaises(ValueError):
                    runtime.extract_source(archive, Path(folder) / 'out')
                self.assertFalse((Path(folder) / 'escape').exists())

    def test_unused_firmware_tree_can_be_omitted_without_extracting_its_host_links(self):
        with tempfile.TemporaryDirectory() as folder:
            archive = Path(folder) / 'source.tar'
            with tarfile.open(archive, 'w') as output:
                link = tarfile.TarInfo('source/roms/host-link')
                link.type = tarfile.SYMTYPE
                link.linkname = '/host/include'
                output.addfile(link)
                code = tarfile.TarInfo('source/main.c'); code.size = 4
                output.addfile(code, io.BytesIO(b'code'))
            runtime.extract_source(archive, Path(folder) / 'out', exclude_prefixes=('source/roms',))
            self.assertEqual((Path(folder) / 'out/source/main.c').read_bytes(), b'code')
            self.assertFalse((Path(folder) / 'out/source/roms').exists())


if __name__ == '__main__':
    unittest.main()
