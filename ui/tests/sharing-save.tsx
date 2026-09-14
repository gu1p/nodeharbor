// Public browser fixture. Infrastructure is simulated; App and its save flow are production code.
import { createRoot } from 'react-dom/client';
import { App } from '../src/App';
import { defaultPolicy, type Backend, type Snapshot } from '../src/model';
import '../src/styles.css';
const policy=defaultPolicy();
const initial:Snapshot={deviceId:'browser-fixture',name:'Test computer',platform:'linux',architecture:'amd64',version:'fixture',state:'paused',reason:'Ready for setup',enrolled:true,controllerUrl:'',workloads:[],worker:{installed:false,running:false},resources:{cpus:8,memoryMib:16384,diskGib:27},policy,configuration:{revision:1,consent:false,policy,hardware:null,allocatedResources:null,capabilities:{storage:true,storageReason:'',localApproval:[]},storageInventory:[],receipts:[]},storage:{revision:1,supported:true,reason:'',defaultDirectory:'/managed/storage',systemDisk:{directory:'/managed',allocationGib:16},locations:[],volumes:[{id:'first',label:'Work SSD',mountPoint:'/work',filesystem:'ext4',availableGib:200,configuredGib:0,eligible:true,reason:'',suggestedDirectory:'/work/NodeHarbor'},{id:'second',label:'Data HDD',mountPoint:'/data',filesystem:'xfs',availableGib:200,configuredGib:0,eligible:true,reason:'',suggestedDirectory:'/data/NodeHarbor'}]}};
let current:Snapshot=JSON.parse(localStorage.getItem('sharing-fixture')??'null')??initial;
const backend:Backend={mode:'desktop',snapshot:async()=>structuredClone(current),fleet:async()=>[],enroll:async()=>current,
 previewStorage:async selections=>({revision:current.storage!.revision!,locations:selections.map((l,i)=>({...l,id:l.id??`disk${i}`,volumeId:l.expectedVolumeId!})),totalGib:selections.reduce((n,l)=>n+l.allocationGib,0),requiresRestart:false}),
 applyStorage:async()=>{throw new Error('The full sharing save must not use a separate storage write');},
 savePolicy:async(policy,revision,plan)=>{
  if(revision!==current.configuration!.revision)throw new Error('Settings changed; reload current settings');
  current={...current,policy,configuration:{...current.configuration!,policy,revision:revision+1},storage:plan?{...current.storage!,revision:plan.revision+1,locations:plan.locations.map(l=>({...l,available:true,reason:''}))}:current.storage};
  localStorage.setItem('sharing-fixture',JSON.stringify(current));return structuredClone(current);
 },
 action:async action=>{if(action==='prepare'){if(current.storage!.locations.length!==2)throw new Error('Both selected drives must be saved before preparing');current={...current,worker:{installed:true,running:false},reason:'Prepared with saved storage'};}return structuredClone(current);},
};
createRoot(document.getElementById('root')!).render(<App backend={backend}/>);
