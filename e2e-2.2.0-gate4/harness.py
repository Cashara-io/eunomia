import urllib.request,urllib.error,http.cookiejar,json,os,sys,base64,secrets,time
from pathlib import Path
S=Path('/private/tmp/eunomia-gate4'); BASE='http://127.0.0.1:3480'
def save(name,data):
 p=S/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(json.dumps(data,indent=2,ensure_ascii=False));return data
def api(path,body=None,method=None):
 auth=json.loads((S/'credentials.json').read_text())
 h={'Content-Type':'application/json','Cookie':auth['cookie']}
 req=urllib.request.Request(BASE+'/api/'+path,data=json.dumps(body).encode() if body is not None else None,headers=h,method=method)
 try:
  with urllib.request.urlopen(req,timeout=180) as r: return json.load(r)
 except urllib.error.HTTPError as e:return {'http_error':e.code,'body':e.read().decode()}
def sql(q):
 cred=json.loads((S/'credentials.json').read_text());h={'Authorization':'Basic '+base64.b64encode(b'root:root').decode(),'Accept':'application/json','surreal-ns':'eunomia','surreal-db':cred['db']}
 with urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:8100/sql',data=q.encode(),headers=h),timeout=90) as r:return json.load(r)
def mcp(name,args):
 cfg=json.loads((S/'mcp.json').read_text())['mcpServers']['eunomia-local']; h={**cfg['headers'],'Content-Type':'application/json','Accept':'application/json, text/event-stream'}
 req=urllib.request.Request(cfg['url'],data=json.dumps({'jsonrpc':'2.0','id':1,'method':'tools/call','params':{'name':name,'arguments':args}}).encode(),headers=h)
 with urllib.request.urlopen(req,timeout=240) as r:
  raw=r.read().decode()
  try:d=json.loads(raw)
  except: d=json.loads(next(x[6:] for x in raw.splitlines() if x.startswith('data: ')))
 r=d.get('result',d)
 if 'structuredContent' in r:return r['structuredContent']
 if 'content' in r:
  texts=[x['text'] for x in r['content'] if x['type']=='text']
  try:return json.loads('\n'.join(texts))
  except:return texts
 return r
def counts(label):return save('ev/counts-'+label+'.json',sql('SELECT count() FROM entity GROUP ALL; SELECT count() FROM entity_property GROUP ALL; SELECT count() FROM relates_to GROUP ALL; SELECT count() FROM memory GROUP ALL;'))
if __name__=='__main__':
 if sys.argv[1]=='setup':
  email='gate4-'+str(int(time.time()))+'@example.com';pw=secrets.token_hex(16)
  jar=http.cookiejar.CookieJar();op=urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
  for path,body in [('auth/register',{'email':email,'password':pw}),('settings/complete-onboarding',{})]:
   with op.open(urllib.request.Request(BASE+'/api/'+path,data=json.dumps(body).encode(),headers={'Content-Type':'application/json'}),timeout=180) as r:r.read()
  cookie='; '.join(c.name+'='+c.value for c in jar)
  h={'Authorization':'Basic '+base64.b64encode(b'root:root').decode(),'Accept':'application/json','surreal-ns':'eunomia','surreal-db':'control'}
  with urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:8100/sql',data=b'INFO FOR NS;',headers=h)) as r:dbs=json.load(r)[0]['result']['databases']
  db=next(x for x in dbs if x.startswith('org_'))
  p=S/'credentials.json';p.touch(mode=0o600);save('credentials.json',dict(email=email,password=pw,cookie=cookie,db=db))
  token=api('auth/tokens',{'name':'gate4-test-mcp'})['token'];p=S/'mcp.json';p.touch(mode=0o600)
  save('mcp.json',{'mcpServers':{'eunomia-local':{'type':'http','url':'http://127.0.0.1:8481/mcp','headers':{'Authorization':'Bearer '+token}}}})
  counts('start');footy=mcp('vault_create',{'name':'Freo Rowing Club'});save('ev/footy-create.json',footy)
  for name,text in [('Training','Rowing training is Tuesdays at Fremantle harbour at 6 am.'),('Club fees','The annual club fee is $95.'),('Saturday match','The next regatta is on the Swan River on Saturday.')]:
   save('ev/footy-'+name.replace(' ','-')+'.json',mcp('memory_write',{'vault_id':'Freo Rowing Club','subject_name':name,'subject_kind':'topic','text':text}))
  print('Fresh admin, protected MCP config and unrelated org memories created.')
 elif sys.argv[1]=='counts':print(json.dumps(counts(sys.argv[2])))
 elif sys.argv[1]=='sql':print(json.dumps(sql(sys.argv[2]),indent=2))
 elif sys.argv[1]=='api':print(json.dumps(api(sys.argv[2]),indent=2))
 elif sys.argv[1]=='mcp':print(json.dumps(mcp(sys.argv[2],json.loads(sys.argv[3])),indent=2))
