import { afterEach, describe, expect, it, vi } from 'vitest';
import { defaultPolicy, type StoragePlan } from './model';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';

afterEach(() => { clearMocks(); vi.resetModules(); });
describe('The native desktop bridge', () => {
  it('previews selected storage and applies the exact reviewed plan through native commands', async () => {
    const selections=[{id:'disk1',directory:'/Volumes/Work SSD/NodeHarbor',allocationGib:40}];
    const plan:StoragePlan={revision:7,locations:[{...selections[0],volumeId:'volume1'}],totalGib:40,requiresRestart:true};
    const commands:{cmd:string;payload:unknown}[]=[];
    mockIPC((cmd,payload)=>{commands.push({cmd,payload});return cmd==='preview_storage'?plan:{state:'draining'};});
    const {backend}=await import('./backend');
    expect(await backend.previewStorage!(selections)).toEqual(plan);
    expect(await backend.applyStorage!(plan)).toEqual({state:'draining'});
    expect(commands).toEqual([
      {cmd:'preview_storage',payload:{selections}},
      {cmd:'apply_storage',payload:{plan}},
    ]);
  });

  it('returns the native folder selection without changing it and treats cancellation as no selection', async () => {
    const commands:string[]=[];
    let selection:string|null='/Volumes/Work SSD/Projetos/ação';
    mockIPC(cmd=>{commands.push(cmd);return selection;});
    const {backend}=await import('./backend');
    expect(await backend.chooseStorageDirectory!()).toBe(selection);
    selection=null;
    expect(await backend.chooseStorageDirectory!()).toBeNull();
    expect(commands).toEqual(['choose_storage_directory','choose_storage_directory']);
  });

  it('does not expose host folder selection or local storage writes in the fleet browser', async () => {
    const previous=Object.getOwnPropertyDescriptor(window,'__TAURI_INTERNALS__');
    Reflect.deleteProperty(window,'__TAURI_INTERNALS__');
    try {
      const {backend}=await import('./backend');
      expect(backend.mode).toBe('fleet');
      expect(backend.chooseStorageDirectory).toBeUndefined();
      expect(backend.previewStorage).toBeUndefined();
      expect(backend.applyStorage).toBeUndefined();
    } finally {
      if(previous)Object.defineProperty(window,'__TAURI_INTERNALS__',previous);
    }
  });

  it('routes worker controls through native commands without sending credentials to the UI', async () => {
    const commands: { cmd: string; payload: unknown }[] = [];
    mockIPC((cmd, payload) => { commands.push({ cmd, payload }); return { state: 'paused' }; });
    const { backend } = await import('./backend');
    expect(backend.mode).toBe('desktop');
    await backend.snapshot();
    await (backend as typeof backend & {activity:()=>Promise<unknown>}).activity();
    await backend.action('pause');
    await backend.enroll('https://workers.example.com', 'one-use-code');
    expect(commands).toEqual([
      { cmd: 'snapshot', payload: {} },
      { cmd: 'activity', payload: {} },
      { cmd: 'worker_action', payload: { action: 'pause' } },
      { cmd: 'enroll', payload: { url: 'https://workers.example.com', code: 'one-use-code' } },
    ]);
  });
});

it('passes explicitly confirmed replacement through the native worker command',async()=>{
 const commands: {cmd:string;payload:unknown}[]=[];
 mockIPC((cmd,payload)=>{commands.push({cmd,payload}); return {state:'paused'};});
 const {backend}=await import('./backend');
 await (backend as typeof backend & {recreateWorker:(policy:ReturnType<typeof defaultPolicy>)=>Promise<unknown>}).recreateWorker(defaultPolicy());
 expect(commands).toEqual([{cmd:'recreate_worker',payload:{policy:defaultPolicy()}}]);
});

it('exposes update status and owner controls only through the native updater',async()=>{
 const commands:{cmd:string;payload:unknown}[]=[];
 mockIPC((cmd,payload)=>{commands.push({cmd,payload});return {};});
 const {backend}=await import('./backend');await backend.updates!();await backend.updateAction!('disable');
 expect(commands).toEqual([{cmd:'update_status',payload:{}},{cmd:'update_action',payload:{action:'disable'}}]);
});
