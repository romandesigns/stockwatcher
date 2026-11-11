'use strict';
// The ONLY executable task exposed to Claude. No shell, network, arbitrary code or input paths.
const fs=require('node:fs');
const path=require('node:path');
const readline=require('node:readline');
const assert=require('node:assert/strict');
const {TASK,CASES,EXPECTED,sha,evaluate,verifyReview,exactKeys}=require('./core.cjs');
const root=path.join(__dirname,'.runtime');
const read=name=>fs.readFileSync(path.join(root,name),'utf8');
const json=name=>JSON.parse(read(name));
function execute(args) {
  exactKeys(args,['task_id','artifact_sha256']);
  assert(!fs.existsSync(path.join(root,'STOP')),'STOP is set');
  const state=json('state.json'); assert.equal(state.stage,'claude-executing');
  const artifact=read('recipe.json'); const hash=sha(artifact); const reviewText=read('review.json');
  assert.equal(args.task_id,TASK); assert.equal(args.artifact_sha256,hash);
  verifyReview(JSON.parse(reviewText),hash);
  const approval=json('approval.json');
  assert.equal(approval.task_id,TASK); assert.equal(approval.artifact_sha256,hash);
  assert.equal(approval.review_sha256,sha(reviewText)); assert.equal(approval.scope,'synthetic-fixed-fixture-only');
  const event=json('event-'+state.seq+'.json');
  const ack=json('ack-'+state.seq+'.json'); assert.equal(ack.event_sha256,sha(read('event-'+state.seq+'.json')));
  assert.equal(event.stage,'claude-executing');
  const result=CASES.map(input=>evaluate(JSON.parse(artifact),input)); assert.deepEqual(result,EXPECTED);
  const receipt={task_id:TASK,artifact_sha256:hash,review_sha256:sha(reviewText),case_results:result,passed:6,actor:'claude-executor-via-fixed-mcp',time:new Date().toISOString()};
  const dest=path.join(root,'execution-receipt.json');
  if(fs.existsSync(dest)) { const old=json('execution-receipt.json'); assert.equal(old.artifact_sha256,hash); assert.equal(old.review_sha256,sha(reviewText)); return old; }
  fs.writeFileSync(dest,JSON.stringify(receipt,null,2)+'\n',{flag:'wx'}); return receipt;
}
const respond=(id,result)=>process.stdout.write(JSON.stringify({jsonrpc:'2.0',id,result})+'\n');
readline.createInterface({input:process.stdin}).on('line',line=>{
  let msg;
  try {
    if(line.length>32768) throw new Error('message too large'); msg=JSON.parse(line);
    if(msg.id===undefined) return;
    if(msg.method==='initialize') return respond(msg.id,{protocolVersion:msg.params.protocolVersion,capabilities:{tools:{}},serverInfo:{name:'stockwatcher-synthetic-fixture',version:'1.0.0'}});
    if(msg.method==='ping') return respond(msg.id,{});
    if(msg.method==='tools/list') return respond(msg.id,{tools:[{name:'execute_approved_fixture',description:'Execute the approved, hash-bound synthetic arithmetic fixture. Only six built-in test inputs; no shell, network, filesystem paths or arbitrary code.',inputSchema:{type:'object',properties:{task_id:{type:'string',const:TASK},artifact_sha256:{type:'string',pattern:'^[a-f0-9]{64}$'}},required:['task_id','artifact_sha256'],additionalProperties:false}}]});
    if(msg.method==='tools/call') {
      assert.equal(msg.params.name,'execute_approved_fixture');
      const result=execute(msg.params.arguments);
      return respond(msg.id,{content:[{type:'text',text:JSON.stringify(result)}],isError:false});
    }
    process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:msg.id,error:{code:-32601,message:'Method not found'}})+'\n');
  } catch(e) {
    if(msg?.id!==undefined) respond(msg.id,{content:[{type:'text',text:'Fixture denied: '+e.message}],isError:true});
  }
});
