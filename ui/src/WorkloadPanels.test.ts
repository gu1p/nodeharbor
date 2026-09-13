import { describe, expect, it } from 'vitest';
import { groupWorkloads, systemComponentTitle } from './WorkloadPanels';
import type { Workload } from './model';

describe('Workload presentation',()=>{
 it('groups system namespaces without treating a similarly named user job as infrastructure',()=>{
  const probe={name:'nodeharbor-probe-abc12',namespace:'nodeharbor-system',state:'running'};
  const job={...probe,namespace:'nodeharbor-ci'};
  const customJob={...probe,namespace:'nodeharbor-system-backups'};
  const kube={name:'coredns-abc12',namespace:'kube-system',state:'running'};
  const input: readonly Workload[]=Object.freeze([probe,job,customJob,kube]);
  const grouped=groupWorkloads(input);
  expect(grouped.workloads).toEqual([job,customJob]);
  expect(grouped.systemComponents).toEqual([probe,kube]);
  expect(input).toEqual([probe,job,customJob,kube]);
 });

 it('gives only the NodeHarbor probe a friendly title and preserves unfamiliar names',()=>{
  const probe={name:'nodeharbor-probe-abc12',namespace:'nodeharbor-system',state:'running'};
  expect(systemComponentTitle(probe)).toBe('NodeHarbor health check');
  for(const other of [
   {...probe,namespace:'nodeharbor-ci'},
   {...probe,name:'nodeharbor-probe-helper-backup'},
   {...probe,name:'another-system-helper'},
  ]) expect(systemComponentTitle(other)).toBe(other.name);
 });
});
