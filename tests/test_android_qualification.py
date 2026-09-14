import copy
import importlib.util
from pathlib import Path
import sys
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
spec = importlib.util.spec_from_file_location('android_qualification', ROOT / 'scripts/qualify_android.py')
qa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qa)
OWNER = '00000000-0000-4000-8000-000000000001'
IMAGE = 'registry.example/ci@sha256:' + 'a' * 64


class AndroidQualificationContract(unittest.TestCase):
    def test_repeating_qualification_resets_only_an_explicitly_disposable_newer_installation(self):
        device = Mock()
        device.installed_version.return_value = qa.android_version_code('0.2.76')
        device.vpn.return_value = 'unchanged'
        with patch.object(qa, 'certificate', return_value='c' * 64):
            qa.signed_upgrade(device, Path('old.apk'), Path('old-tests.apk'), Path('new.apk'),
                              Path('new-tests.apk'), '0.2.76', reset_test_installation=True)
        device.reset_test_installation.assert_called_once_with('c' * 64)
        device.reset_mock()
        device.installed_version.return_value = qa.android_version_code('0.2.75')
        with patch.object(qa, 'certificate', return_value='c' * 64):
            qa.signed_upgrade(device, Path('old.apk'), Path('old-tests.apk'), Path('new.apk'),
                              Path('new-tests.apk'), '0.2.76', reset_test_installation=True)
        device.reset_test_installation.assert_not_called()

    def test_real_job_requires_normal_scheduler_qualification_and_only_contributed_toleration(self):
        job = qa.ci_job('nodeharbor-test-123', 'workers', OWNER, IMAGE)
        pod = job['spec']['template']['spec']
        self.assertNotIn('nodeName', pod)
        self.assertEqual(pod['nodeSelector']['kubernetes.io/arch'], 'arm64')
        self.assertEqual(pod['nodeSelector']['nodeharbor.node-restriction.kubernetes.io/ci'], 'true')
        self.assertEqual(pod['nodeSelector']['nodeharbor.sikalio.dev/device'], OWNER)
        self.assertEqual(pod['tolerations'], [{'key':'nodeharbor.sikalio.dev/contributed', 'operator':'Equal', 'value':'true', 'effect':'NoSchedule'}])
        self.assertFalse(pod['automountServiceAccountToken'])
        self.assertNotIn('hostNetwork', pod)

    def test_success_requires_the_actual_pod_on_this_owned_arm64_node(self):
        job = {'metadata': {'name': 'job', 'uid': 'job-uid'}, 'status': {'succeeded': 1, 'conditions': [{'type':'Complete', 'status':'True'}]}}
        pod = {'metadata': {'ownerReferences': [{'uid':'job-uid', 'kind':'Job'}]},
               'spec': {'nodeName':'nodeharbor-'+OWNER.replace('-', ''), 'containers':[{'name':'ci', 'image':IMAGE}]},
               'status': {'phase':'Succeeded', 'containerStatuses':[{'name':'ci', 'state':{'terminated':{'exitCode':0}}}]}}
        qa.verify_job(job, pod, OWNER, IMAGE)
        for invalid in [dict(pod, spec=dict(pod['spec'], nodeName='different-node')),
                        dict(pod, status=dict(pod['status'], phase='Running')),
                        dict(pod, metadata={'ownerReferences':[]})]:
            with self.assertRaises(ValueError): qa.verify_job(job, invalid, OWNER, IMAGE)

    def test_upgrade_verifies_both_signers_before_installing_and_preserves_data_and_vpn(self):
        device = Mock()
        device.vpn.return_value = 'unchanged'
        with patch.object(qa, 'certificate', return_value='c'*64):
            qa.signed_upgrade(device, Path('old.apk'), Path('old-tests.apk'), Path('new.apk'), Path('new-tests.apk'), '0.1.12')
        self.assertEqual(device.install.call_args_list[0].args, (Path('old.apk'), Path('old-tests.apk')))
        self.assertEqual(device.install.call_args_list[1].args, (Path('new.apk'), Path('new-tests.apk')))
        self.assertEqual(device.instrument.call_args_list[0].args[0], ['SignedUpdateContract#seed'])
        self.assertEqual(device.instrument.call_args_list[1].args[0], ['SignedUpdateContract#verify'])
        device.reset_mock()
        with patch.object(qa, 'certificate', side_effect=['a'*64, 'b'*64, 'a'*64, 'a'*64]):
            with self.assertRaises(ValueError):
                qa.signed_upgrade(device, Path('old.apk'), Path('old-tests.apk'), Path('new.apk'), Path('new-tests.apk'), '0.1.12')
        device.install.assert_not_called()
