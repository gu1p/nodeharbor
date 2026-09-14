import type { StorageInventory, StorageSelection } from './model';
export interface StorageDraft {
 revision:number;
 locations:StorageSelection[];
 saved:StorageSelection[];
 temporaryDirectory:string;
 singleDiskGib:number;
 savedSingleDiskGib:number;
}
export function createStorageDraft(inventory:StorageInventory):StorageDraft {
 const locations=inventory.locations.map(({id,volumeId,directory,allocationGib})=>({id,expectedVolumeId:volumeId,directory,allocationGib}));
 const singleDiskGib=inventory.configuredGib||30;
 return {revision:inventory.revision??0,locations,saved:locations,temporaryDirectory:'',singleDiskGib,savedSingleDiskGib:singleDiskGib};
}
export function storageDraftDirty(draft:StorageDraft):boolean {
 return JSON.stringify(draft.locations)!==JSON.stringify(draft.saved)||draft.singleDiskGib!==draft.savedSingleDiskGib;
}
export function refreshStorageDraft(draft:StorageDraft|null,inventory:StorageInventory):StorageDraft {
 if(draft&&storageDraftDirty(draft))return draft;
 if(draft&&draft.revision===(inventory.revision??0))return draft;
 return {...createStorageDraft(inventory),temporaryDirectory:draft?.temporaryDirectory??''};
}
