#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { spawnSync } from 'node:child_process';

const cases = {
  'video-fractional-rgba-pcm': {
    appearance:['native_lossless_mov_preserves_every_pixel_sample_channel_and_exact_clock','client_encodings_preserve_fractional_clocks_channel_order_and_decoded_signal'],
    failure:['delivery_refuses_incomplete_stale_unbounded_cancelled_and_invalid_work'],
    preservation:[],
  },
  'video-editable-timeline': {
    editability:['undo_redo_save_recovery_preserve_groups_and_stale_commands_fail'],
    undo:['undo_redo_save_recovery_preserve_groups_and_stale_commands_fail'],
    save_reopen:['undo_redo_save_recovery_preserve_groups_and_stale_commands_fail'],
    failure:['overlap_overflow_invalid_ranges_and_recursive_insert_leave_document_untouched'],
    preservation:[],
  },
  'video-movable-archive': {
    editability:['moved_archives_reopen_save_and_recover_without_the_original_media'],
    save_reopen:['moved_archives_reopen_save_and_recover_without_the_original_media'],
    failure:['moved_archives_reopen_save_and_recover_without_the_original_media'],
    preservation:[],
  },
};
if (process.argv[2] === '--plan') {
  console.log(JSON.stringify({product:'editbay',tier:'headless',cases,boundary:'native media libraries and document tests; no desktop/display/audio-device/physical proof'}));
} else {
  const request = JSON.parse(fs.readFileSync(process.argv[2],'utf8'));
  const plan = cases[request.fixture.id];
  if (!plan || request.driver !== 'libtest' || !request.binary) throw Error('Known case and exact integration-test executable required');
  const digest = () => crypto.createHash('sha256').update(fs.readFileSync(request.input)).digest('hex');
  const before = digest();
  const results = {};
  for (const name of [...new Set(Object.values(plan).flat())]) {
    const run = spawnSync(request.binary,['--exact',name,'--nocapture','--test-threads=1'],{encoding:'utf8',timeout:25000,maxBuffer:65536,env:process.env});
    const output = `${run.stdout ?? ''}${run.stderr ?? ''}`;
    const log = `test-${Object.keys(results).length}.log`;
    fs.writeFileSync(path.join(request.output,log),output);
    results[name] = {passed:run.status===0 && !run.error && /test result: ok\. 1 passed; 0 failed; 0 ignored;/.test(output),artifact:log};
    if (!results[name].passed) break;
  }
  const unchanged = before===digest();
  fs.writeFileSync(path.join(request.output,'assertions.json'),JSON.stringify({fixture:request.fixture.id,tests:results,input_unchanged:unchanged,boundary:'Existing production delivery, typed timeline and movable archive assertions; headless only. FFmpeg writer/reader share codecs; independent codec conformance is not established.'},null,2));
  const checks = Object.fromEntries(Object.entries(plan).map(([key,names])=>[key,{status:(key==='preservation'?unchanged:names.every(n=>results[n]?.passed))?'passed':'failed',artifacts:['assertions.json',...names.flatMap(n=>results[n]?[results[n].artifact]:[])]}]));
  fs.writeFileSync(path.join(request.output,'result.json'),JSON.stringify({checks}));
  process.exitCode=Object.values(checks).every(c=>c.status==='passed')?0:1;
}
