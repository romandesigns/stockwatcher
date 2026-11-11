'use strict';
const assert=require('node:assert/strict');const P=require('./policy.cjs');
class Engine {
 constructor(store,token,io){Object.assign(this,{store,token,io});}
 check(t){this.store.fence(this.token);this.store.verify();P.grant(t.grant,t.spec);assert(Date.now()-t.created<P.LIMITS.taskMs,'task deadline');this.io.check(t);}
 async tick(id){const s=this.store,t=s.get(id);this.check(t);await this.io.github.identity();await this.io.github.source(t.spec.source_sha);await this.io.github.flush(s);this.check(t);const move=(to,actor,decision,extra)=>s.transition(id,to,this.token,actor,decision,extra);
 if(t.state==='queued')return move('planning','codex','Create fixed read-only declarative plan.');
 if(t.state==='changes_requested'){if(t.attempt>=P.LIMITS.maxRetries)return move('blocked','coordinator','Remediation budget exhausted.');return move('planning','codex','Bounded remediation requested.',{attempt:t.attempt+1});}
 if(t.state==='planning'){
 const n='plan-'+t.attempt;let result;
 if(s.hasArtifact(id,n))result=s.readArtifact(id,n).value;
 else if(s.hasArtifact(id,'intent-'+n))return move('blocked','coordinator','Ambiguous planning after interruption; not re-executed.');
 else {s.tx(()=>{s.fence(this.token);s.artifact(id,'intent-'+n,{time:Date.now()});});result=await this.io.agents.plan(t,t.attempt?s.readArtifact(id,'review-'+(t.attempt-1)).value.findings:[]);this.check(t);s.tx(()=>{s.fence(this.token);s.artifact(id,n,P.plan(result,t.spec));});}
 P.plan(result,t.spec);return move('planned','codex','Plan artifact validated.',{artifacts:{...t.artifacts,plan:s.readArtifact(id,n).hash}});}
 if(t.state==='planned')return move('reviewing','claude-reviewer','Independent review of exact plan hash.');
 if(t.state==='reviewing'){
 const plan=s.readArtifact(id,'plan-'+t.attempt);assert.equal(plan.hash,t.artifacts.plan);const n='review-'+t.attempt;let result;
 if(s.hasArtifact(id,n))result=s.readArtifact(id,n).value;
 else if(s.hasArtifact(id,'intent-'+n))return move('blocked','coordinator','Ambiguous review after interruption; not re-executed.');
 else {s.tx(()=>{s.fence(this.token);s.artifact(id,'intent-'+n,{time:Date.now()});});result=await this.io.agents.review(t,plan);this.check(t);s.tx(()=>{s.fence(this.token);s.artifact(id,n,P.review(result,t.spec,plan.hash));});}
 P.review(result,t.spec,plan.hash);return move(result.verdict==='ACCEPT'?'approved':'changes_requested','claude-reviewer',result.verdict,{artifacts:{...t.artifacts,review:s.readArtifact(id,n).hash}});}
 if(t.state==='approved'){const plan=s.readArtifact(id,'plan-'+t.attempt),review=s.readArtifact(id,'review-'+t.attempt);assert.equal(plan.hash,t.artifacts.plan);assert.equal(review.hash,t.artifacts.review);P.review(review.value,t.spec,plan.hash);assert.equal(review.value.verdict,'ACCEPT');return move('executing','claude-executor','Owner grant + independent ACCEPT authorize only fixed read-only inventory.');}
 if(t.state==='executing'){
 const plan=s.readArtifact(id,'plan-'+t.attempt);
 if(!s.hasArtifact(id,'execution')){
 if(s.hasArtifact(id,'intent-executor'))return move('blocked','coordinator','Execution outcome unknown; no automatic repeat.');
 s.tx(()=>{s.fence(this.token);s.artifact(id,'intent-executor',{time:Date.now(),plan:plan.hash});});
 const narrative=await this.io.agents.execute(t,plan);this.check(t);s.tx(()=>{s.fence(this.token);s.artifact(id,'executor-narrative',narrative);});}
 const receipt=s.readArtifact(id,'execution');assert.equal(receipt.value.task_id,id);assert.equal(receipt.value.plan_sha256,t.artifacts.plan);assert.equal(receipt.value.review_sha256,t.artifacts.review);assert.equal(receipt.value.source_sha,t.spec.source_sha);assert.equal(receipt.value.grant_sha256,P.hash(t.grant));assert.equal(receipt.value.inventory_sha256,s.readArtifact(id,'inventory').hash);return move('completed','coordinator','Fixed read-only execution receipt verified; remote-state limitations retained.',{artifacts:{...t.artifacts,execution:receipt.hash,inventory:receipt.value.inventory_sha256}});}
 assert(['completed','blocked'].includes(t.state));return null;
 }
}
module.exports={Engine};
