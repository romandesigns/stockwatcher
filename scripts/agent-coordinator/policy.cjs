'use strict';
const assert=require('node:assert/strict');const crypto=require('node:crypto');
const REPO='romandesigns/stockwatcher',ISSUE=64,BRANCH='codex/issue64-durable-coordinator';
const ACTIONS=['inventory_reports','inventory_backup_metadata','recorder_status'];
const LIMITS={agentMs:180000,taskMs:1200000,leaseMs:30000,heartbeatMs:5000,maxRetries:2,maxOutput:600000,minFreeBytes:2*1024**3,pollMs:5000};
const sha=v=>crypto.createHash('sha256').update(v).digest('hex');
function canonical(v){if(Array.isArray(v))return '['+v.map(canonical).join(',')+']';if(v&&typeof v==='object')return '{'+Object.keys(v).sort().map(k=>JSON.stringify(k)+':'+canonical(v[k])).join(',')+'}';return JSON.stringify(v);}
const hash=v=>sha(canonical(v));
function keys(v,ks){assert(v&&typeof v==='object'&&!Array.isArray(v));assert.deepEqual(Object.keys(v).sort(),ks.slice().sort(),'unknown or missing fields');}
function spec(v){keys(v,['id','repository','issue','kind','source_sha']);assert(/^sw64-[a-z0-9-]{1,48}$/.test(v.id));assert.equal(v.repository,REPO);assert.equal(v.issue,ISSUE);assert.equal(v.kind,'preservation_inventory');assert(/^[a-f0-9]{40}$/.test(v.source_sha));return v;}
function grant(v,s,now=Date.now()){keys(v,['task_sha256','scope','expires_at','authorized_by','authorization_source']);assert.equal(v.task_sha256,hash(spec(s)));assert.equal(v.scope,'preservation-read-only');assert.equal(v.authorized_by,'romandesigns');assert.equal(v.authorization_source,'owner-chat-2026-11-11-issue64-resume');assert(Number.isFinite(Date.parse(v.expires_at))&&Date.parse(v.expires_at)>now,'authorization expired');assert(Date.parse(v.expires_at)-now<=24*3600000,'authorization lifetime too long');return v;}
function plan(v,s){keys(v,['task_id','actions','summary']);assert.equal(v.task_id,s.id);assert.deepEqual(v.actions,ACTIONS);assert(typeof v.summary==='string'&&v.summary.length<=2000);return v;}
function review(v,s,artifact){keys(v,['task_id','artifact_sha256','verdict','findings']);assert.equal(v.task_id,s.id);assert.equal(v.artifact_sha256,artifact);assert(['ACCEPT','CHANGES_REQUESTED'].includes(v.verdict));assert(Array.isArray(v.findings)&&v.findings.length<=20&&v.findings.every(x=>typeof x==='string'&&x.length<=2000));if(v.verdict==='ACCEPT')assert.equal(v.findings.length,0);return v;}
function redact(v){return String(v).replace(/(?:sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{15,}|github_pat_[A-Za-z0-9_]{15,})/g,'[REDACTED]').replace(/Bearer\s+[^\s"']+/gi,'Bearer [REDACTED]');}
module.exports={REPO,ISSUE,BRANCH,ACTIONS,LIMITS,sha,hash,canonical,keys,spec,grant,plan,review,redact};
