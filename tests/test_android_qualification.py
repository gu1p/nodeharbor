import copy
import importlib.util
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
spec = importlib.util.spec_from_file_location('android_qualification', ROOT / 'scripts/qualify_android.py')
qa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qa)
OWNER = '00000000-0000-4000-8000-000000000001'
IMAGE = 'registry.example/ci@sha256:' + 'a' * 64


class AndroidQualificationContract(unittest.TestCase):
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
