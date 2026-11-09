import json,urllib.request,http.cookiejar,time,statistics,pathlib,subprocess,os,sys,math,random
from urllib.parse import quote
root=pathlib.Path('/tmp/eunomia-ontology2-v2')
binary=sys.argv[1]
cases=[('kinds',''),('products','?kind=product'),('stock-name','?kind=stock_record'),('stock-page-2','?kind=stock_record&offset=25'),('stock-deep','?kind=stock_record&offset=11950'),('stock-sort','?kind=stock_record&sort=units&descending=true'),('stock-filter','?kind=stock_record&sort=units&filter=units&op=gte&value=11900'),('sales','?kind=weekly_sales'),('sales-sort','?kind=weekly_sales&sort=units&descending=true'),('profile',None)]
results={label:{'cold_ms':[],'warm_ms':[]} for label,_ in cases}
def start(case):
 info=root/'fixture.json'
 env=dict(os.environ,ONTOLOGY_FIXTURE_DIR=str(root),ONTOLOGY_FIXTURE_PORT='18860',RUST_MIN_STACK='8388608',TEST_HARDENED='1',TEST_SURREAL_URL='ws://127.0.0.1:18644/rpc',ONTOLOGY_REUSE_FIXTURE='1')
 log=open(root/'runtime.log','w');p=subprocess.Popen([binary,'--exact','serve_banksia_sized_fixture','--ignored','--nocapture'],cwd=os.getcwd(),env=env,stdout=log,stderr=subprocess.STDOUT)
 try:
  deadline=time.monotonic()+90
  while True:
   if p.poll() is not None:raise RuntimeError((root/'runtime.log').read_text())
   if time.monotonic()>deadline:raise RuntimeError('Fixture startup timeout')
   try:
    if info.exists():
     urllib.request.urlopen('http://127.0.0.1:18860/healthz',timeout=1).close();break
   except Exception: pass
   time.sleep(.05)
  info=json.loads(info.read_text());jar=http.cookiejar.CookieJar();op=urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
  req=urllib.request.Request('http://127.0.0.1:18860/api/auth/login',data=json.dumps({'email':'tester@example.com','password':'correct-horse-battery'}).encode(),headers={'Content-Type':'application/json'})
  with op.open(req) as r:r.read()
  return p,log,op,info
 except BaseException:p.terminate();p.wait();log.close();raise

def read(op,info,label,suffix):
 base='http://127.0.0.1:18860/api/vaults/'+quote(info['space'])+'/ontology/objects'
 suffix='/'+quote(info['object']) if suffix is None else suffix
 t=time.perf_counter()
 with op.open(base+suffix,timeout=120) as r:payload=r.read();data=json.loads(payload)
 ms=round((time.perf_counter()-t)*1000,3)
 expected={'products':300,'stock-name':12000,'stock-page-2':12000,'stock-deep':12000,'stock-sort':12000,'stock-filter':100,'sales':6240,'sales-sort':6240}
 if label in expected:assert data['total']==expected[label],(label,data)
 if label in ['stock-sort','stock-filter','sales-sort']:
  expected_first={'stock-sort':11999,'stock-filter':11900,'sales-sort':6239}[label]
  assert data['rows'][0]['values']['units']['value']==expected_first,(label,data)
 if label=='kinds':assert sum(k['entities'] for k in data['kinds'])==18878
 if label=='profile':
  number=next(n for n in data['numbers'] if n['name']=='gross_profit')['result']['run']
  assert abs(number['value']-1100)<1e-9 and number['n_entities']==1,number
 return ms

def save():
 for r in results.values():
  for mode in ['cold','warm']:
   a=r[mode+'_ms']
   if a:r[mode]={'samples':len(a),'p50_ms':round(statistics.median(a),3),'p95_ms':sorted(a)[math.ceil(len(a)*.95)-1]}
 (root/'performance.json').write_text(json.dumps({'method':'Serial loopback HTTP including response read, JSON decode and exact counts. SurrealDB 3.3.5 with production hardened capabilities and RocksDB on a disposable Docker container. Cold: first object endpoint after independently restarting both the database and backend for every case/sample, reopening the same 18878-object fixture without reseeding. Warm: one backend/database process, one priming request per case then 50 reads. No OS disk-cache flush.','results':results},indent=2))
for i in range(50):
 for label,suffix in cases:
  subprocess.run(['docker','restart','eunomia-ontology2-v2-hardened'],check=True,capture_output=True)
  deadline=time.monotonic()+30
  while True:
   try:urllib.request.urlopen('http://127.0.0.1:18644/health',timeout=1).close();break
   except Exception:
    if time.monotonic()>deadline:raise
    time.sleep(.1)
  p,log,op,info=start(label)
  try:results[label]['cold_ms'].append(read(op,info,label,suffix))
  finally:p.terminate();p.wait();log.close()
 save();print('Cold round',i+1,{k:v['cold']['p50_ms'] for k,v in results.items()},flush=True)
p,log,op,info=start('warm')
try:
 for label,suffix in cases:
  read(op,info,label,suffix)
  for i in range(50):results[label]['warm_ms'].append(read(op,info,label,suffix))
  save();print('Warm',label,results[label]['warm'],flush=True)
finally:p.terminate();p.wait();log.close()
