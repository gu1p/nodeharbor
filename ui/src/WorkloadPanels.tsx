import { Box, ShieldCheck } from 'lucide-react';
import type { Workload } from './model';

// Presentation only: the agent and controller still own all drain decisions.
export function groupWorkloads(items: readonly Workload[]) {
 const workloads: Workload[]=[];
 const systemComponents: Workload[]=[];
 for(const item of items) {
  const system=item.namespace==='nodeharbor-system'||item.namespace==='kube-system';
  (system?systemComponents:workloads).push(item);
 }
 return {workloads,systemComponents};
}

export function systemComponentTitle(component: Workload) {
 return component.namespace==='nodeharbor-system'&&/^nodeharbor-probe-[a-z0-9]+$/.test(component.name)
  ?'NodeHarbor health check':component.name;
}

export function WorkloadPanels({items}:{items:readonly Workload[]}) {
 const {workloads,systemComponents}=groupWorkloads(items);
 return <>
  <section className="panel" aria-labelledby="running-workloads-title">
   <div className="section-heading small"><h2 id="running-workloads-title">Running workloads</h2><span className="pill">{workloads.length} active</span></div>
   {workloads.length?<div className="workload-list">{workloads.map(workload=><div key={`${workload.namespace}/${workload.name}`}>
    <Box size={19} aria-hidden="true"/><strong className="workload-identity">{workload.name}<small>{workload.namespace}</small></strong><span className="pill">{workload.state}</span>
   </div>)}</div>:<div className="empty"><Box size={30} aria-hidden="true"/><h3>No workloads running</h3><p>Work appears here when your worker is connected and eligible.</p></div>}
  </section>
  <section className="panel" aria-labelledby="system-components-title">
   <div className="section-heading small"><h2 id="system-components-title">System components</h2><span className="pill">{systemComponents.length} running</span></div>
   <p>Health checks and helpers that keep your worker connected.</p>
   {systemComponents.length?<div className="workload-list">{systemComponents.map(component=>{
    const title=systemComponentTitle(component);
    return <div key={`${component.namespace}/${component.name}`}>
     <ShieldCheck size={19} aria-hidden="true"/>
     <div className="workload-identity"><strong>{title}</strong>
      {title!==component.name&&<small>Checks your worker’s connection to the cluster.</small>}
      <details><summary>Component details</summary>{title!==component.name&&<small>{component.name}</small>}<small>{component.namespace}</small></details>
     </div>
     <span className="pill">{component.state}</span>
    </div>;
   })}</div>:<div className="empty"><ShieldCheck size={30} aria-hidden="true"/><h3>No system components running</h3><p>System components appear here when the worker starts.</p></div>}
  </section>
 </>;
}
