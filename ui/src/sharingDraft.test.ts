import { expect, it } from 'vitest';
import { createStorageDraft, refreshStorageDraft, storageDraftDirty } from './sharingDraft';
import type { StorageInventory } from './model';
const inventory:StorageInventory={revision:2,defaultDirectory:'/managed',supported:true,reason:'',volumes:[],locations:[{id:'one',volumeId:'data',directory:'/data/worker',allocationGib:40,available:true,reason:''}]};
it('compares allocation, directory and volume identity against the original saved choices',()=>{
 const draft=createStorageDraft(inventory);
 expect(storageDraftDirty(draft)).toBe(false);
 for(const change of [{allocationGib:50},{directory:'/data/new'},{expectedVolumeId:'replacement'}]) {
  expect(storageDraftDirty({...draft,locations:[{...draft.locations[0],...change}]})).toBe(true);
 }
 expect(storageDraftDirty({...draft,temporaryDirectory:'/backup'})).toBe(false);
});
it('retains edited choices and their original revision when newer inventory arrives',()=>{
 const draft=createStorageDraft(inventory);draft.locations=[{...draft.locations[0],allocationGib:100}];
 const result=refreshStorageDraft(draft,{...inventory,revision:3,locations:[]});
 expect(result).toBe(draft);expect(result.revision).toBe(2);
 expect(result.locations[0].allocationGib).toBe(100);
});
it('refreshes a clean draft after committed changes while preserving a backup folder',()=>{
 const draft={...createStorageDraft(inventory),temporaryDirectory:'/backup'};
 const refreshed=refreshStorageDraft(draft,{...inventory,revision:3,locations:[]});
 expect(refreshed.revision).toBe(3);expect(refreshed.locations).toEqual([]);
 expect(refreshed.temporaryDirectory).toBe('/backup');expect(storageDraftDirty(refreshed)).toBe(false);
});
it('opens pending selections as the saved draft without replacing the applied inventory',()=>{
 const pendingUpdate={totalGib:100,layout:{version:1,systemLocationId:'one',volumeId:'data',runtimeDirectory:'/data/new/.nh12345678',systemGib:16},locations:[{id:'one',volumeId:'data',directory:'/data/new',allocationGib:99},{id:'two',volumeId:'other',directory:'/other/worker',allocationGib:1}]};
 const pending=Object.assign({...inventory,revision:3,configuredGib:40},{pendingUpdate});
 const draft=createStorageDraft(pending);
 expect(draft.locations).toEqual([{id:'one',expectedVolumeId:'data',directory:'/data/new',allocationGib:99},{id:'two',expectedVolumeId:'other',directory:'/other/worker',allocationGib:1}]);
 expect(draft.singleDiskGib).toBe(100);expect(storageDraftDirty(draft)).toBe(false);
 expect(pending.locations[0].allocationGib).toBe(40);
 expect(refreshStorageDraft(createStorageDraft(inventory),pending).locations).toEqual(draft.locations);
});
