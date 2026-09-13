import type { Ref } from 'react';
import type { Policy, Resources } from './model';
const days=['Mon','Tue','Wed','Thu','Fri','Sat','Sun'];
const time=(n:number)=>`${String(Math.floor(n/60)).padStart(2,'0')}:${String(n%60).padStart(2,'0')}`;
const minutes=(value:string)=>{const [h,m]=value.split(':').map(Number);return h*60+m;};
export function SharingRules({policy,host,onChange,onSave,busy,locked=false,saveButtonRef}:{policy:Policy;host:Resources;onChange:(policy:Policy)=>void;onSave:()=>void;busy:boolean;locked?:boolean;saveButtonRef?:Ref<HTMLButtonElement>}) {
 const set=<K extends keyof Policy>(key:K,value:Policy[K])=>onChange({...policy,[key]:value});
 const number=(label:string,value:number,update:(n:number)=>void,min:number,max:number,suffix?:string)=><label className="number-field"><span>{label}</span><div><input aria-label={label} type="number" value={value||''} min={min} max={max} onChange={e=>update(Number(e.target.value))}/><small>{suffix}</small></div></label>;
 const toggle=(label:string,description:string,key:'idleOnly'|'allowBattery'|'scheduleEnabled'|'startAtLogin'|'background'|'allowCi'|'allowServices')=><label className="toggle-row"><span><strong>{label}</strong><small>{description}</small></span><input type="checkbox" aria-label={label} checked={policy[key]} onChange={e=>set(key,e.target.checked)}/></label>;
 return <form onSubmit={e=>{e.preventDefault();onSave();}}>
 <div className="section-heading"><div><h1>Sharing rules</h1><p>Choose how much to share, and when your computer is available.</p></div><button className="primary" ref={saveButtonRef} disabled={busy||locked} type="submit">{busy?'Saving…':'Save sharing rules'}</button></div>
 <fieldset disabled={busy||locked} className="rules-fields"><section className="panel"><h2>Your resource budget</h2><p>This allowance includes the Linux worker and its running containers.</p><div className="resource-inputs">
 {number('CPU cores',policy.resources.cpus,n=>set('resources',{...policy.resources,cpus:n}),1,Math.max(1,host.cpus-1),`of ${host.cpus} available`)}
 {number('RAM (GiB)',policy.resources.memoryMib/1024,n=>set('resources',{...policy.resources,memoryMib:Math.round(n*1024)}),2,Math.max(2,Math.floor(host.memoryMib/1024)-2),`of ${Math.round(host.memoryMib/1024)} GiB total`)}
 {number('Disk (GiB)',policy.resources.diskGib,n=>set('resources',{...policy.resources,diskGib:n}),15,Math.max(15,host.diskGib-10),'for the worker VM')}
 </div><p className="hint">Changing CPU or RAM drains work before restarting the worker. Disk shrinking requires recreating it.</p></section>
 <div className="two-columns"><section className="panel"><h2>When to share</h2>
 {toggle('Only while idle','Pause new work when you return to your computer.','idleOnly')}
 {number('Idle time (minutes)',policy.idleAfterMinutes,n=>set('idleAfterMinutes',n),1,1440)}
 {toggle('Allow sharing on battery','Keep contributing when the power cable is disconnected.','allowBattery')}
 {number('Minimum battery (%)',policy.minBatteryPercent,n=>set('minBatteryPercent',n),0,100)}
 {toggle('Use a weekly schedule','Share only inside the windows below, using this computer’s time zone.','scheduleEnabled')}
 {policy.scheduleEnabled&&<div className="schedule-list">{policy.schedule.map((window,index)=><fieldset key={index}><legend>Window {index+1}</legend><div className="weekdays">{days.map((day,d)=><label key={day}><input aria-label={`${day} in window ${index+1}`} type="checkbox" checked={window.days.includes(d)} onChange={e=>set('schedule',policy.schedule.map((w,i)=>i===index?{...w,days:e.target.checked?[...w.days,d]:w.days.filter(n=>n!==d)}:w))}/>{day}</label>)}</div><div className="time-inputs"><label>From<input aria-label={`Start time for window ${index+1}`} type="time" value={time(window.startMinute)} onChange={e=>set('schedule',policy.schedule.map((w,i)=>i===index?{...w,startMinute:minutes(e.target.value)}:w))}/></label><label>Until<input aria-label={`End time for window ${index+1}`} type="time" value={time(window.endMinute)} onChange={e=>set('schedule',policy.schedule.map((w,i)=>i===index?{...w,endMinute:minutes(e.target.value)}:w))}/></label><button type="button" onClick={()=>set('schedule',policy.schedule.filter((_,i)=>i!==index))}>Remove window {index+1}</button></div></fieldset>)}<button type="button" onClick={()=>set('schedule',[...policy.schedule,{days:[0,1,2,3,4],startMinute:540,endMinute:1020}])}>Add sharing window</button>{!policy.schedule.length&&<p>No windows yet. Sharing stays paused until you add one.</p>}</div>}
 </section><div><section className="panel"><h2>Workload types</h2>{toggle('Accept CI jobs','Run compatible build and test jobs.','allowCi')}{toggle('Accept eligible services','Run approved services once this worker qualifies.','allowServices')}<p className="hint">Eligibility also depends on connection quality, reliability, and CPU architecture.</p></section>
 <section className="panel"><h2>App behavior</h2>{toggle('Start at login','Launch NodeHarbor when you sign in.','startAtLogin')}{toggle('Continue when the window closes','Keep the background worker available from the system tray.','background')}{number('Drain deadline (seconds)',policy.drainSeconds,n=>set('drainSeconds',n),0,1800)}<p className="hint">When the deadline expires, remaining work is interrupted.</p></section></div></div>
 </fieldset></form>;
}
