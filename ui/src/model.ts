export interface Resources { cpus:number; memoryMib:number; diskGib:number }
export interface ScheduleWindow { days:number[]; startMinute:number; endMinute:number }
export interface Policy { enabled:boolean; resources:Resources; idleOnly:boolean; idleAfterMinutes:number; allowBattery:boolean; minBatteryPercent:number; scheduleEnabled:boolean; schedule:ScheduleWindow[]; startAtLogin:boolean; background:boolean; allowCi:boolean; allowServices:boolean; drainSeconds:number }
export interface Workload { name:string; namespace:string; state:string; cpu?:string; memory?:string }
export interface Snapshot { deviceId:string; name:string; platform:string; architecture:string; state:string; reason:string; policy:Policy; resources:Resources; worker:{installed:boolean;running:boolean}; enrolled:boolean; controllerUrl:string; version:string; workloads:Workload[] }
export interface Device { deviceId:string; name:string; platform:string; architecture:string; state:string; reason:string; lastSeen:string|null; eligibleCi:boolean; eligibleServices:boolean; resources?:Resources; workloads?:Workload[] }
export type Action = 'prepare'|'resume'|'pause'|'stop';
export interface Backend { mode?:'desktop'|'fleet'; snapshot():Promise<Snapshot>; savePolicy(policy:Policy):Promise<Snapshot>; action(action:Action):Promise<Snapshot>; enroll(url:string,code:string):Promise<Snapshot>; fleet():Promise<Device[]>; createEnrollment?():Promise<{code:string;expiresAt:string}> }
export function defaultPolicy():Policy { return {enabled:false,resources:{cpus:2,memoryMib:4096,diskGib:30},idleOnly:false,idleAfterMinutes:15,allowBattery:false,minBatteryPercent:30,scheduleEnabled:false,schedule:[],startAtLogin:false,background:false,allowCi:true,allowServices:false,drainSeconds:300}; }
