'use strict';
const crypto = require('node:crypto');
const assert = require('node:assert/strict');
const TASK = 'issue63-positive-squares-v1';
const BASE = '3b85ae9ee190edb2bae45860fa7219fe46bf6ae3';
const CASES = [[],[-4,-1,0],[-3,0,1,2,4],[5,5],[1,2,3],[100,-100,0]];
const EXPECTED = [0,0,21,50,14,10000];
const sha = value => crypto.createHash('sha256').update(value).digest('hex');
function exactKeys(value, keys) { assert(value && typeof value === 'object' && !Array.isArray(value)); assert.deepEqual(Object.keys(value).sort(), [...keys].sort()); }
function validateRecipe(recipe) {
  exactKeys(recipe,['task_id','steps']); assert.equal(recipe.task_id,TASK);
  assert(Array.isArray(recipe.steps) && recipe.steps.length === 3);
  for(const step of recipe.steps) { exactKeys(step,['op']); assert(['filter_positive','square','sum'].includes(step.op)); }
  assert.equal(new Set(recipe.steps.map(s=>s.op)).size,3);
}
function evaluate(recipe,input) {
  validateRecipe(recipe);
  assert(Array.isArray(input) && input.length <= 16 && input.every(n=>Number.isSafeInteger(n) && Math.abs(n)<=1000));
  let value=[...input];
  for(const {op} of recipe.steps) {
    assert(Array.isArray(value),'sum must be last');
    if(op==='filter_positive') value=value.filter(n=>n>0);
    else if(op==='square') value=value.map(n=>n*n);
    else value=value.reduce((sum,n)=>sum+n,0);
  }
  assert(Number.isSafeInteger(value)); return value;
}
function verifyReview(review,recipeHash,task=TASK) {
  exactKeys(review,['task_id','artifact_sha256','verdict','case_results','findings']);
  assert.equal(review.task_id,task); assert.equal(review.artifact_sha256,recipeHash);
  assert.equal(review.verdict,'ACCEPT'); assert.deepEqual(review.findings,[]); assert.deepEqual(review.case_results,EXPECTED);
}
function redact(text) {
  return String(text).replace(/(?:sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{15,}|github_pat_[A-Za-z0-9_]{15,})/g,'[REDACTED]').replace(/Bearer\s+[^\s"']+/gi,'Bearer [REDACTED]');
}
module.exports={TASK,BASE,CASES,EXPECTED,sha,exactKeys,validateRecipe,evaluate,verifyReview,redact};
