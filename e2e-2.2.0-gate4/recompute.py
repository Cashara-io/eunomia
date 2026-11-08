import csv,json
from decimal import Decimal as D,ROUND_HALF_UP
from pathlib import Path
s=Path('/private/tmp/eunomia-gate4');rs=list(csv.DictReader((s/'materials/catalogue.csv').open()))
for r in rs:
 for k in ['sell price AUD','unit cost AUD']:r[k]=D(r[k].replace('$','').replace(',','')) if r[k] else None
 r['qty on hand']=D(r['qty on hand'])
def calc(rows):
 valid=[r for r in rows if r['unit cost AUD'] is not None];sell=sum(r['sell price AUD'] for r in valid);cost=sum(r['unit cost AUD'] for r in valid)
 return {'average_sell_price':sum(r['sell price AUD'] for r in rows)/D(len(rows)),'total_stock_at_cost':sum(r['unit cost AUD']*r['qty on hand'] for r in valid),'gross_margin_unweighted_pct':(sell-cost)/sell*100,'costed_rows':len(valid),'total_stock_at_sell':sum(r['sell price AUD']*r['qty on hand'] for r in rows)}
changed=[dict(r,**{'unit cost AUD':r['unit cost AUD']*D('1.1')}) if r['supplier'].lower()=='southern ice works' else r for r in rs]
a=calc(rs);b=calc(changed);data={'overall':a,'shark_bay':calc([r for r in rs if r['supplier']=='Shark Bay Prawns']),'scenario':b,'delta':{k:b[k]-a[k] for k in a if k!='costed_rows'},'money_scenario':b['total_stock_at_cost'].quantize(D('.01'),rounding=ROUND_HALF_UP)}
(s/'ev/recomputed-fixture.json').write_text(json.dumps(data,indent=2,default=str));print(json.dumps(data,indent=2,default=str))
