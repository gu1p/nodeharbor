import importlib.util
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
spec = importlib.util.spec_from_file_location('android_packaging', ROOT / 'scripts/build_android.py')
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


class AndroidPackagingContract(unittest.TestCase):
    def test_official_signer_formats_require_one_consistent_certificate(self):
        digest = 'a' * 64
        for label in ['Signer #1', 'V2 Signer', 'V3.1 Signer']:
            output = f'Number of signers: 1\n{label}: certificate SHA-256 digest: {digest}\n'
            self.assertEqual(build.certificate_digest(output), digest)
        for output in [f'Number of signers: 2\nV2 Signer: certificate SHA-256 digest: {digest}\n',
                       'Number of signers: 1\n',
                       f'Number of signers: 1\nV2 Signer: certificate SHA-256 digest: {digest}\nV3 Signer: certificate SHA-256 digest: '+ 'b'*64 + '\n']:
            with self.assertRaises(ValueError): build.certificate_digest(output)

    def test_package_identity_and_abi_must_match_the_requested_build(self):
        good = "package: name='io.github.gu1p.nodeharbor' versionCode='1000012' versionName='0.1.12'\nnative-code: 'arm64-v8a'\n"
        build.verify_badging(good, '0.1.12')
        for bad in [good.replace('1000012', '1'), good.replace('0.1.12', '0.1.11'),
                    good.replace("'arm64-v8a'", "'arm64-v8a' 'x86_64'"), good.replace('nodeharbor', 'another')]:
            with self.assertRaises(ValueError): build.verify_badging(bad, '0.1.12')
