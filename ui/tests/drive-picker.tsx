import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { StorageLocations } from '../src/StorageLocations';
import type { StorageSelection } from '../src/model';
function Fixture() {
 const [locations,setLocations]=useState<StorageSelection[]>([]);
 return <StorageLocations inventory={{supported:true,reason:'',defaultDirectory:'/managed/storage',locations:[],volumes:[
 {id:'primary',label:'System SSD',mountPoint:'/',filesystem:'ext4',availableGib:100,configuredGib:16,driveType:'ssd',suggestedDirectory:'/managed/storage',eligible:true,reason:''},
 {id:'external',label:'Work HDD',mountPoint:'/media/work',filesystem:'xfs',availableGib:400,configuredGib:0,driveType:'hdd',suggestedDirectory:'/media/work/NodeHarbor',eligible:true,reason:''},
 ]}} locations={locations} onChange={setLocations}/>;
}
createRoot(document.getElementById('root')!).render(<Fixture/>);
