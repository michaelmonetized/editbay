import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const plan=JSON.parse(spawnSync(process.execPath,[path.join(root,'scripts/suite-qualification.mjs'),'--plan'],{encoding:'utf8'}).stdout);
const base=plan.product==='editbay'?'artifacts':'.artifacts';
fs.mkdirSync(path.join(root,base),{recursive:true});
const scratch=fs.mkdtempSync(path.join(root,base,'suite-negative-'));

test.after(()=>fs.rmSync(scratch,{recursive:true,force:true}));
test('exit zero without a single actual test cannot qualify a fixture; missing executable also fails',()=>{
  fs.writeFileSync(path.join(scratch,'source'),'original fixture');
  const request={fixture:{id:Object.keys(plan.cases)[0]},input:path.join(scratch,'source'),output:scratch,driver:'libtest',binary:'/usr/bin/true'};
  const file=path.join(scratch,'request.json');
  for(const binary of ['/usr/bin/true',path.join(scratch,'missing-executable')]){
    fs.writeFileSync(file,JSON.stringify({...request,binary}));
    const result=spawnSync(process.execPath,[path.join(root,'scripts/suite-qualification.mjs'),file],{encoding:'utf8'});
    assert.notEqual(result.status,0);
    const report=JSON.parse(fs.readFileSync(path.join(scratch,'result.json')));
    assert.equal(report.checks.appearance.status,'failed');
    assert.equal(fs.readFileSync(request.input,'utf8'),'original fixture');
  }
});
