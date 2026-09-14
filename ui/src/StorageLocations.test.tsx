import { useState } from 'react';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { StorageEditor, StorageLocations } from './StorageLocations';
import { App } from './App';
import { defaultPolicy, type Backend, type Snapshot } from './model';

const inventory = {
 defaultDirectory:'/Users/owner/Library/Application Support/NodeHarbor/storage',
 supported:true, reason:'',
 volumes:[
  {id:'system',label:'Macintosh HD',mountPoint:'/',filesystem:'apfs',availableGib:100,configuredGib:0,eligible:true,reason:'',driveType:'ssd',suggestedDirectory:'/Users/owner/Library/Application Support/NodeHarbor/storage'},
  {id:'external',label:'Work SSD',mountPoint:'/Volumes/Work SSD',filesystem:'apfs',availableGib:400,configuredGib:40,eligible:true,reason:'',driveType:'ssd',suggestedDirectory:'/Volumes/Work SSD/NodeHarbor'},
 ],
 locations:[],
};
type Selection = {id?:string;directory:string; allocationGib:number};
function Editor({initial=[]}:{initial?:Selection[]}) {
 const [locations,setLocations]=useState(initial);
 return <StorageLocations inventory={inventory} locations={locations} onChange={setLocations} disabled={false}/>;
}

it('shows the resolved default and understandable volume capacities before setup',()=>{
 render(<Editor/>);
 const region=screen.getByRole('region',{name:'Storage locations'});
 expect(within(region).getByText(inventory.defaultDirectory)).toBeVisible();
 expect(within(region).getByText('Macintosh HD')).toBeVisible();
 expect(within(region).getByText('Work SSD')).toBeVisible();
 expect(region).toHaveTextContent('/Volumes/Work SSD');
 expect(region).toHaveTextContent('400 GiB available');
 expect(region).toHaveTextContent('40 GiB configured');
 expect(region).toHaveTextContent(/drain.*restart/i);
 expect(region).toHaveTextContent(/interrupted.*deadline/i);
});

it('lets an owner enter separate directories and allocations using labeled controls',async()=>{
 const user=userEvent.setup();render(<Editor/>);
 await user.click(screen.getByRole('button',{name:'Add drive'}));
 await user.selectOptions(screen.getByRole('combobox',{name:'Drive for disk 1'}),'external');
 await user.clear(screen.getByRole('textbox',{name:'Directory for disk 1'}));
 await user.type(screen.getByRole('textbox',{name:'Directory for disk 1'}),'/Volumes/Work SSD/nodeharbor');
 const first=screen.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'});
 await user.clear(first);await user.type(first,'80');
 await user.click(screen.getByRole('button',{name:'Add drive'}));
 await user.selectOptions(screen.getByRole('combobox',{name:'Drive for disk 2'}),'system');
 await user.clear(screen.getByRole('textbox',{name:'Directory for disk 2'}));
 await user.type(screen.getByRole('textbox',{name:'Directory for disk 2'}),'/Users/owner/worker-data');
 expect(first).toHaveValue(80);
 expect(screen.getByRole('textbox',{name:'Directory for disk 1'})).toHaveValue('/Volumes/Work SSD/nodeharbor');
 await user.click(screen.getByRole('button',{name:'Remove disk 2'}));
 expect(screen.queryByRole('textbox',{name:'Directory for disk 2'})).not.toBeInTheDocument();
});

it('keeps unavailable configured storage visible and explains unsupported runtime capability',()=>{
 const onChange=vi.fn();
 render(<StorageLocations inventory={{...inventory,supported:false,reason:'This runtime does not support selectable disk locations.',locations:[{id:'saved',volumeId:'external',directory:'/Volumes/Work SSD/nodeharbor',allocationGib:40,available:false,reason:'Volume unavailable; reconnect Work SSD.'}]}} locations={[]} onChange={onChange} disabled={false}/>);
 expect(screen.getByRole('alert')).toHaveTextContent('Volume unavailable; reconnect Work SSD.');
 expect(screen.getByText('/Volumes/Work SSD/nodeharbor')).toBeVisible();
 expect(screen.getByText('This runtime does not support selectable disk locations.')).toBeVisible();
 expect(screen.getByRole('button',{name:'Add drive'})).toBeDisabled();
 expect(onChange).not.toHaveBeenCalled();
});

it('exposes host inventory from the desktop snapshot on the sharing rules page',async()=>{
 const snapshot:Snapshot={deviceId:'test',name:'Computer',platform:'macos',architecture:'arm64',state:'paused',reason:'Sharing is switched off',policy:defaultPolicy(),resources:{cpus:8,memoryMib:16384,diskGib:100},storage:{...inventory,supported:false,reason:'This runtime does not support selectable disk locations.'},worker:{installed:false,running:false},enrolled:false,controllerUrl:'',version:'test',workloads:[]};
 const backend:Backend={snapshot:vi.fn().mockResolvedValue(snapshot),savePolicy:vi.fn().mockResolvedValue(snapshot),action:vi.fn().mockResolvedValue(snapshot),enroll:vi.fn().mockResolvedValue(snapshot),fleet:vi.fn().mockResolvedValue([])};
 const user=userEvent.setup();render(<App backend={backend}/>);
 await user.click(await screen.findByRole('button',{name:'Sharing rules'}));
 expect(screen.getByRole('region',{name:'Storage locations'})).toHaveTextContent('Work SSD');
 expect(screen.getByRole('button',{name:'Add drive'})).toBeDisabled();
 expect(screen.getByRole('button',{name:'Save sharing rules'})).toBeEnabled();
});

it('offers a smaller allocation and an explicit delete-all action for the last disk',()=>{
 const saved={id:'disk1',volumeId:'external',directory:'/Volumes/Work SSD/nodeharbor',allocationGib:40,available:true,reason:''};
 render(<StorageLocations inventory={{...inventory,locations:[saved]}} locations={[saved]} onChange={vi.fn()}/>);
 expect(screen.getByRole('textbox',{name:'Directory for disk 1'})).toBeEnabled();
 expect(screen.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'})).toHaveAttribute('min','1');
 expect(screen.getByRole('button',{name:'Shrink allocation for disk 1'})).toBeEnabled();
 expect(screen.getByRole('button',{name:'Delete all worker storage'})).toBeEnabled();
 expect(screen.queryByRole('button',{name:'Remove disk 1'})).not.toBeInTheDocument();
 expect(screen.getByRole('region',{name:'Storage locations'})).not.toHaveTextContent('cannot be moved');
});

it('allows removing a member while keeping the other disk selected',async()=>{
 const user=userEvent.setup();const onChange=vi.fn();
 const locations=[{id:'one',volumeId:'external',directory:'/one',allocationGib:40,available:true,reason:''},{id:'two',volumeId:'system',directory:'/two',allocationGib:30,available:true,reason:''}];
 render(<StorageLocations inventory={{...inventory,locations}} locations={locations} onChange={onChange}/>);
 await user.click(screen.getByRole('button',{name:'Remove disk 1'}));
 expect(onChange).toHaveBeenCalledWith([locations[1]]);
});

it('requires explicit data-loss confirmation in the delete-all review',async()=>{
 const user=userEvent.setup();const applyStorage=vi.fn();
 const backend={previewStorage:vi.fn().mockResolvedValue({revision:1,locations:[],totalGib:0,requiresRestart:true,maintenance:{kind:'deleteAll',backup:null,minimumGib:0,temporaryBytes:0,deletions:['All worker data'],downtime:'Worker storage remains disabled'}}),applyStorage} as unknown as Backend;
 render(<StorageEditor inventory={{...inventory,locations:[{id:'one',volumeId:'external',directory:'/one',allocationGib:40,available:true,reason:''}]}} backend={backend} disabled={false}/>);
 await user.click(screen.getByRole('button',{name:'Delete all worker storage'}));
 expect(await screen.findByRole('checkbox',{name:/delete all worker data/i})).not.toBeChecked();
 expect(screen.getByRole('button',{name:'Confirm deletion'})).toBeDisabled();
 expect(screen.getByText(/host enrollment remains/i)).toBeVisible();
 expect(applyStorage).not.toHaveBeenCalled();
 await user.click(screen.getByRole('checkbox',{name:/delete all worker data/i}));
 await user.click(screen.getByRole('button',{name:'Confirm deletion'}));
 expect(applyStorage).toHaveBeenCalledOnce();
});

it('keeps deletion available for retired copies after worker storage is disabled',()=>{
 const copy={id:'old',volumeId:'external',directory:'/old',allocationGib:40,available:false,reason:'Reconnect the original volume'};
 render(<StorageLocations inventory={{...inventory,disabled:true,retainedCopies:[copy]}} locations={[]} onChange={vi.fn()} onDeleteAll={vi.fn()}/>);
 expect(screen.getByRole('button',{name:'Delete all worker storage'})).toBeEnabled();
 expect(screen.getByText(/reconnect.*delete all worker storage/i)).toBeVisible();
});

it('shows the actual single-disk allocation in the Windows storage summary',()=>{
 render(<StorageLocations inventory={{...inventory,supported:false,configuredGib:15}} locations={[]}/>);
 expect(screen.getByText(/15 GiB allocated/i)).toBeVisible();
});

it('keeps the recovery backup and its occupied space visible while restore is interrupted',()=>{
 const recovery={...inventory,recoveryBackup:{path:'/backup/recovery',bytes:2147483648,verified:true},operation:{phase:'Restore',message:'Restore interrupted'}};
 render(<StorageEditor inventory={recovery} disabled={false}/>);
 expect(screen.getByText('/backup/recovery')).toBeVisible();
 expect(screen.getByText(/2.00 GiB.*keep this file/i)).toBeVisible();
});

it('explains automatic recovery consent and distinguishes active from configured capacity',()=>{
 render(<StorageEditor inventory={{...inventory,activeGib:30,configuredGib:70,recoveryEnabled:false} as typeof inventory} disabled={false}/>);
 expect(screen.getByText(/30 GiB active.*70 GiB configured/i)).toBeVisible();
 expect(screen.getByRole('checkbox',{name:/automatically recover.*missing disk/i})).not.toBeChecked();
 expect(screen.getByText(/discard.*entire.*pool/i)).toBeVisible();
 expect(screen.getByText(/Kubernetes retry policies/i)).toBeVisible();
});

it('explains combined capacity and the effect of disconnecting a selected disk',()=>{
 render(<Editor initial={[{directory:'/one',allocationGib:30},{directory:'/two',allocationGib:40}]}/>);
 const region=screen.getByRole('region',{name:'Storage locations'});
 expect(region).toHaveTextContent('70 GiB allocated');
 expect(region).toHaveTextContent(/usable.*less.*reserved/i);
 expect(region).toHaveTextContent(/disconnect.*whole worker/i);
});

it('offers review before applying a storage change from sharing rules',async()=>{
 const snapshot:Snapshot={deviceId:'test',name:'Computer',platform:'macos',architecture:'arm64',state:'paused',reason:'Sharing is switched off',policy:defaultPolicy(),resources:{cpus:8,memoryMib:16384,diskGib:100},storage:inventory,worker:{installed:false,running:false},enrolled:false,controllerUrl:'',version:'test',workloads:[]};
 const previewStorage=vi.fn().mockResolvedValue({revision:0,locations:[{id:'disk1',volumeId:'external',directory:'/Volumes/Work SSD/nodeharbor',allocationGib:40}],totalGib:40,requiresRestart:false});
 const applyStorage=vi.fn().mockResolvedValue(snapshot);
 const backend:Backend & {previewStorage:typeof previewStorage;applyStorage:typeof applyStorage}={snapshot:vi.fn().mockResolvedValue(snapshot),savePolicy:vi.fn().mockResolvedValue(snapshot),action:vi.fn().mockResolvedValue(snapshot),enroll:vi.fn().mockResolvedValue(snapshot),fleet:vi.fn().mockResolvedValue([]),previewStorage,applyStorage};
 const user=userEvent.setup();render(<App backend={backend}/>);
 await user.click(await screen.findByRole('button',{name:'Sharing rules'}));
 await user.click(screen.getByRole('button',{name:'Add drive'}));
 await user.selectOptions(screen.getByRole('combobox',{name:'Drive for disk 1'}),'external');
 await user.clear(screen.getByRole('textbox',{name:'Directory for disk 1'}));
 await user.type(screen.getByRole('textbox',{name:'Directory for disk 1'}),'/Volumes/Work SSD/nodeharbor');
 const allocation=screen.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'});
 await user.clear(allocation);await user.type(allocation,'40');
 await user.click(screen.getByRole('button',{name:'Review storage changes'}));
 expect(previewStorage).toHaveBeenCalledWith([{directory:'/Volumes/Work SSD/nodeharbor',allocationGib:40,expectedVolumeId:'external'}]);
 expect(applyStorage).not.toHaveBeenCalled();
 await user.click(await screen.findByRole('button',{name:'Apply storage changes'}));
 expect(applyStorage).toHaveBeenCalledOnce();
});

it('selects mounted drives with suggested folders and independent allocations in tab order',async()=>{
 const user=userEvent.setup();render(<Editor/>);
 await user.tab();expect(screen.getByRole('button',{name:'Add drive'})).toHaveFocus();
 await user.keyboard('{Enter}');
 const first=screen.getByRole('combobox',{name:'Drive for disk 1'});
 expect(first).toHaveFocus();
 await user.selectOptions(first,'system');
 expect(screen.getByRole('textbox',{name:'Directory for disk 1'})).toHaveValue(inventory.defaultDirectory);
 await user.tab();expect(screen.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'})).toHaveFocus();
 await user.keyboard('{ControlOrMeta>}a{/ControlOrMeta}45');
 await user.tab();expect(screen.getByRole('textbox',{name:'Directory for disk 1'})).toHaveFocus();
 await user.tab();await user.tab();expect(screen.getByRole('button',{name:'Add drive'})).toHaveFocus();
 await user.keyboard('{Enter}');
 const second=screen.getByRole('combobox',{name:'Drive for disk 2'});expect(second).toHaveFocus();
 await user.selectOptions(second,'external');
 expect(screen.getByRole('textbox',{name:'Directory for disk 2'})).toHaveValue('/Volumes/Work SSD/NodeHarbor');
 expect(screen.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'})).toHaveValue(45);
 expect(screen.getByRole('spinbutton',{name:'Allocation for disk 2 (GiB)'})).toHaveValue(30);
});

it('requires a drive identity before review and keeps saved identities after reopening',async()=>{
 const user=userEvent.setup();const previewStorage=vi.fn();
 const saved={id:'one',volumeId:'external',directory:'/Volumes/Work SSD/custom',allocationGib:50,available:true,reason:''};
 const {unmount}=render(<StorageEditor inventory={{...inventory,locations:[saved]}} backend={{previewStorage,applyStorage:vi.fn()} as unknown as Backend} disabled={false}/>);
 expect(screen.getByRole('combobox',{name:'Drive for disk 1'})).toHaveValue('external');
 await user.click(screen.getByRole('button',{name:'Review storage changes'}));
 expect(previewStorage).toHaveBeenCalledWith([{id:'one',directory:saved.directory,allocationGib:50,expectedVolumeId:'external'}]);
 unmount();previewStorage.mockClear();
 render(<StorageEditor inventory={inventory} backend={{previewStorage,applyStorage:vi.fn()} as unknown as Backend} disabled={false}/>);
 await user.click(screen.getByRole('button',{name:'Add drive'}));
 await user.click(screen.getByRole('button',{name:'Review storage changes'}));
 expect(previewStorage).not.toHaveBeenCalled();
 expect(screen.getByRole('alert')).toHaveTextContent(/select a drive/i);
});

it('can discard a storage draft to save unrelated sharing rules',async()=>{
 const initial:Snapshot={deviceId:'test',name:'Computer',platform:'linux',architecture:'amd64',state:'paused',reason:'Sharing is switched off',policy:defaultPolicy(),resources:{cpus:8,memoryMib:16384,diskGib:100},storage:inventory,worker:{installed:false,running:false},enrolled:true,controllerUrl:'',version:'test',workloads:[]};
 const api:Backend={snapshot:vi.fn().mockResolvedValue(initial),savePolicy:vi.fn().mockResolvedValue(initial),action:vi.fn(),enroll:vi.fn(),fleet:vi.fn().mockResolvedValue([]),previewStorage:vi.fn(),applyStorage:vi.fn()};
 const user=userEvent.setup();render(<App backend={api}/>);
 await user.click(await screen.findByRole('button',{name:'Sharing rules'}));
 await user.click(screen.getByRole('checkbox',{name:'Only while idle'}));
 await user.click(screen.getByRole('button',{name:'Add drive'}));
 await user.click(screen.getByRole('button',{name:'Discard storage changes'}));
 expect(screen.queryByRole('combobox',{name:'Drive for disk 1'})).not.toBeInTheDocument();
 expect(screen.getByRole('button',{name:'Save sharing rules'})).toBeEnabled();
 await user.click(screen.getByRole('button',{name:'Save sharing rules'}));
 expect(api.savePolicy).toHaveBeenCalledWith(expect.objectContaining({idleOnly:true}));
 expect(api.applyStorage).not.toHaveBeenCalled();
});
