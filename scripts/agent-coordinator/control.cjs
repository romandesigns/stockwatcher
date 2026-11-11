'use strict';
const fs=require('node:fs');const path=require('node:path');const cp=require('node:child_process');const assert=require('node:assert/strict');const {Store}=require('./store.cjs');const P=require('./policy.cjs');const H=require('./host.cjs');const {GitHub}=require('./github.cjs');const {Agents}=require('./agents.cjs');const {Engine}=require('./engine.cjs');
const ROOT=path.join(__dirname,'.state');const sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function main(){const cmd=process.argv[2]||'status';const c=H.readConfig(ROOT);const s=new Store(path.join(ROOT,'queue.sqlite'));let keep=false;
 const stopped=()=>{assert(!fs.existsSync(path.join(ROOT,'STOP')),'STOP');assert.equal(s.getMeta('enabled'),'true','disabled');};
 try{
 if(cmd==='init'){s.verify();s.setMeta('enabled','false');fs.writeFileSync(path.join(ROOT,'STOP'),'Default disabled\n');console.log('Initialized disabled');return;}
 if(cmd==='status'){console.log(JSON.stringify({enabled:s.getMeta('enabled')==='true',stop:fs.existsSync(path.join(ROOT,'STOP')),lease:s.db.prepare('SELECT pid,expires FROM lease').get()||null,tasks:s.tasks().map(t=>({id:t.spec.id,state:t.state,attempt:t.attempt,source_sha:t.spec.source_sha})),events:s.events().length,receipts:s.db.prepare('SELECT count(*) AS n FROM receipts').get().n,last_error:s.getMeta('last_error')||null},null,2));return;}
 if(cmd==='stop'){fs.writeFileSync(path.join(ROOT,'STOP'),'Operator stop '+new Date().toISOString()+'\n');s.setMeta('enabled','false');console.log('STOP enabled');return;}
 if(cmd==='enqueue'){assert(process.argv[3],'authorization file required');const authorization=JSON.parse(fs.readFileSync(process.argv[3]));P.keys(authorization,['spec','grant']);H.source(c,authorization.spec.source_sha);P.grant(authorization.grant,authorization.spec);const gh=new GitHub(c,ROOT);await gh.identity();const issue=await gh.api('repos/'+P.REPO+'/issues/64');assert.equal(issue.user.login,'romandesigns');assert.equal(issue.state,'open');const t=s.enqueue(authorization.spec,authorization.grant);if(!s.hasArtifact(t.spec.id,'issue-source'))s.artifact(t.spec.id,'issue-source',{number:issue.number,body:issue.body,author:issue.user.login,body_sha256:P.sha(issue.body),observed_at:new Date().toISOString()});console.log(JSON.stringify({id:t.spec.id,state:t.state}));return;}
 if(cmd==='start'||cmd==='restart'){
 assert(process.argv.includes('--supervised'),'explicit supervised mode required');
 if(cmd==='restart'){fs.writeFileSync(path.join(ROOT,'STOP'),'Restart requested\n');s.setMeta('enabled','false');for(let i=0;i<60&&s.db.prepare('SELECT 1 FROM lease').get();i++)await sleep(500);}
 const prior=s.db.prepare('SELECT * FROM lease').get();if(prior){try{process.kill(prior.pid,0);throw new Error('existing worker is alive');}catch(e){assert.equal(e.code,'ESRCH','existing or ambiguous worker; cannot restart');}assert(prior.expires<Date.now(),'prior lease not expired');}
 s.verify();assert(s.tasks().some(t=>!['completed','blocked'].includes(t.state)),'no authorized queued work');for(const t of s.tasks().filter(t=>!['completed','blocked'].includes(t.state))){P.grant(t.grant,t.spec);H.source(c,t.spec.source_sha);}
 await new GitHub(c,ROOT).identity();s.setMeta('enabled','true');if(fs.existsSync(path.join(ROOT,'STOP')))fs.unlinkSync(path.join(ROOT,'STOP'));
 const out=fs.openSync(path.join(ROOT,'worker.log'),'a'),err=fs.openSync(path.join(ROOT,'worker.stderr.log'),'a');const args=[__filename,'worker'];if(process.argv.includes('--until-idle'))args.push('--until-idle');const child=cp.spawn(c.binaries.node.path,args,{cwd:c.checkout,env:H.env(c),windowsHide:true,detached:true,stdio:['ignore',out,err]});child.unref();fs.closeSync(out);fs.closeSync(err);console.log(JSON.stringify({pid:child.pid,mode:'supervised-read-only'}));return;}
 if(cmd==='worker'){
 stopped();const token=s.acquire(process.pid);let lost=null;const heartbeat=setInterval(()=>{try{s.heartbeat(token);}catch(e){lost=e;}},P.LIMITS.heartbeatMs);const check=()=>{if(lost)throw lost;stopped();s.fence(token);};const gh=new GitHub(c,ROOT,check);const agents=new Agents(c,ROOT,s,token,check);const engine=new Engine(s,token,{github:gh,agents,check:t=>{check();H.source(c,t.spec.source_sha);}});
 try{while(true){check();s.verify();await gh.identity();await gh.flush(s);const t=s.tasks().find(t=>!['completed','blocked'].includes(t.state));if(t){await engine.tick(t.spec.id);await gh.flush(s);}else break;}}
 catch(e){s.setMeta('last_error',P.redact(e.message));throw e;}
 finally{clearInterval(heartbeat);s.setMeta('enabled','false');fs.writeFileSync(path.join(ROOT,'STOP'),'Worker stopped '+new Date().toISOString()+'\n');s.release(token);}return;}
 throw new Error('Unknown command');
 }finally{if(!keep)s.close();}}
main().catch(e=>{console.error(P.redact(e.stack||e.message));process.exitCode=1;});
