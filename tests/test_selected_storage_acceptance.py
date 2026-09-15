"""Packaged GUI evidence must check the total allocation and actual VM location."""
import copy
import sys
import unittest
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import desktop_storage_acceptance as acceptance

class SelectedStorageEvidence(unittest.TestCase):
    def test_saved_physical_disks_are_checked_against_the_total_owner_selection(self):
        saved = {'storageLocations': [
            {'id':'primary', 'directory':'/data/NodeHarbor', 'volumeId':'picked', 'allocationGib':14},
            {'id':'second', 'directory':'/other/NodeHarbor', 'volumeId':'other', 'allocationGib':40},
        ], 'storageLayout': {'version':1,'systemLocationId':'primary','volumeId':'picked','systemGib':16,'runtimeDirectory':'/data/NodeHarbor/.nh12345678'}}
        self.assertEqual(acceptance.saved_total_locations(saved), [('/data/NodeHarbor',30,'picked'),('/other/NodeHarbor',40,'other')])
        for key, value in [('volumeId','wrong'), ('runtimeDirectory','/settings/lima'), ('systemLocationId','missing')]:
            wrong=copy.deepcopy(saved);wrong['storageLayout'][key]=value
            with self.assertRaises(AssertionError): acceptance.saved_total_locations(wrong)

if __name__ == '__main__': unittest.main()
