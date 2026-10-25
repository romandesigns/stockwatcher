import json,sys,statistics as st,collections,random
rows=[json.loads(l) for l in open(sys.argv[1])]
by=collections.defaultdict(dict)
for r in rows: by[r['rule']][(r['s'],r['t0'])]=r
print('rules',{k:len(v) for k,v in by.items()})
days=sorted({r['day'] for r in rows}); print('days',len(days))
trig={k for v in by.values() for k in v}; print('triggers',len(trig))
def m(x): return sum(x)/len(x) if x else float('nan')
for k in ['B0','B1','B2','B3']+[r for r in by if r.startswith('C')]:
    v=list(by[k].values())
    kinds=collections.Counter(r['kindq'] for r in v)
    print(k,len(v),'meanq %.4f'%m([r['netq'] for r in v]),'gross %.4f'%m([r['gross'] for r in v]),
      'med hin %.4f'%st.median([r['hin'] for r in v if r['hin'] is not None]),
      {a:round(b/len(v),3) for a,b in kinds.items()}, 'med delay %.0f'%st.median([r['delay'] for r in v]))
def paired(a,b,f='netq',boot=2000):
    keys=[k for k in by[a] if k in by[b]]
    d=collections.defaultdict(list)
    for k in keys: d[by[a][k]['day']].append(by[a][k][f]-by[b][k][f])
    dl=list(d); pt=m([x for l in d.values() for x in l])
    bs=[]
    for _ in range(boot):
        s=[x for dd in (random.choice(dl) for _ in dl) for x in d[dd]]; bs.append(m(s))
    bs.sort(); return len(keys),pt,bs[int(.025*boot)],bs[int(.975*boot)]
C=[r for r in by if r.startswith('C2')][0]
for a,b in [(C,'B0'),(C,'B1'),('B1','B0')]:
    for f in ['netq','gross']:
        n,p,lo,hi=paired(a,b,f); print(a,'vs',b,f,n,'%.4f [%.4f, %.4f]'%(p,lo,hi))
# B0 triggers lacking B1
miss=[k for k in by['B0'] if k not in by['B1']]; print('B0 w/o B1',len(miss),'their B0 mean',m([by['B0'][k]['netq'] for k in miss]))
# opening prints
op=[r for r in by['B0'].values() if (r['t0']//10**9)%86400 in range(8*3600,8*3600+120) or (r['t0']//10**9)%86400 in range(9*3600,9*3600+120)]
print('B0 04:00-04:02 (EDT/EST)',len(op),len(op)/len(by['B0']))
