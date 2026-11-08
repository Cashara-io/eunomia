import runpy,json,csv,random,collections,datetime
from pathlib import Path
h=runpy.run_path('/private/tmp/eunomia-gate3/harness.py');sql=h['sql'];save=h['save'];s=Path('/private/tmp/eunomia-gate3')
data=save('ev/independent-db.json',sql("SELECT * FROM entity; SELECT id,name,'organisation' AS kind,vault FROM organisation; SELECT id,name,'person' AS kind,vault FROM person; SELECT * FROM entity_property; SELECT * FROM ontology_property; SELECT * FROM relates_to; SELECT * FROM memory; SELECT * FROM document; SELECT * FROM vault_member;"))
assert all(x['status']=='OK' for x in data)
ents=data[0]['result']+data[1]['result']+data[2]['result'];props={x['id']:x['name'] for x in data[4]['result']}; pv=collections.defaultdict(dict)
for p in data[3]['result']:pv[p['entity']][props[p['property']]]=p.get('value_num',p.get('value_date',p.get('value_text')))
rows=list(csv.DictReader((s/'materials/stock.csv').open()));mismatches=[];out=[]
for r in rows:
 es=[e for e in ents if e['name']==r['item'] and e['vault']=='vault:2oxb9ism79xia4hivfdo'];assert len(es)==1,(r['item'],es)
 e=es[0];expect={'sell_price':float(r['sell price AUD'].replace('$','').replace(',','')),'unit_cost':float(r['unit cost AUD'].replace('$','').replace(',','')) if r['unit cost AUD'] else None,'qty_on_hand':int(r['qty on hand']),'reorder_at':int(r['reorder at']),'status':r['status'],'last_received':datetime.datetime.strptime(r['last received'],'%d/%m/%Y').strftime('%Y-%m-%d')}
 actual=pv[e['id']]
 for k,v in expect.items():
  a=actual.get(k)
  if k=='last_received' and isinstance(a,str):a=a[:10]
  if v!=a:mismatches.append({'name':r['item'],'property':k,'expected':v,'actual':a})
 links=[l for l in data[5]['result'] if l['in']==e['id'] and l['label']=='supplied_by'];sup=[x for x in ents if any(x['id']==l['out'] for l in links)]
 if len(sup)!=1 or sup[0]['name'].casefold()!=r['supplier'].casefold():mismatches.append({'name':r['item'],'supplier':sup})
 out.append({'name':r['item'],'expected':expect,'actual':actual,'supplier':sup})
dup=collections.defaultdict(list)
for e in ents:
 if e['vault']=='vault:2oxb9ism79xia4hivfdo':dup[e['name'].casefold()].append({'id':e['id'],'kind':e['kind'],'name':e['name']})
duplicates={k:v for k,v in dup.items() if len(v)>1}
random.seed(220);sample=random.sample(out,3)+[out[0],out[6]]
foreign=[e for e in ents if e['vault']!='vault:2oxb9ism79xia4hivfdo']
summary={'verified_rows':len(rows),'mismatches':mismatches,'sample_five':sample,'duplicate_names':duplicates,'other_spaces_entities':foreign,'supplier_entities':[e for e in ents if e['name'].casefold() in {r['supplier'].casefold() for r in rows}],'memberships':data[8]['result']}
save('ev/import-verify.json',summary);print(json.dumps(summary,indent=2)[:7500])
