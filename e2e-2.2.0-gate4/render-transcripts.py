from pathlib import Path
from PIL import Image,ImageDraw,ImageFont
import json,textwrap
s=Path('/private/tmp/eunomia-gate4');font=ImageFont.truetype('/System/Library/Fonts/Menlo.ttc',16);title=ImageFont.truetype('/System/Library/Fonts/Supplemental/Arial.ttf',25)
for p in sorted(s.glob('p*/*.jsonl')):
 final=p.with_suffix('.txt').read_text();calls=json.loads(p.with_suffix('.tools.json').read_text());lines=[]
 text='Final answer\n\n'+final+'\n\nMCP call evidence\n'
 for t in calls:
  r=t.get('result',{});short={k:r[k] for k in ['error','code','vault_name','coverage','chosen_by','metric','value','unit','n_entities','n_changed','warnings','created','updated','linked','targets_created','baseline','scenario'] if isinstance(r,dict) and k in r}
  args={k:v for k,v in t['arguments'].items() if k not in ['text','items','kinds','properties','metrics','links']}
  text+='\n'+t['tool']+'\n'+json.dumps(args,ensure_ascii=False)+'\n'+json.dumps(short,ensure_ascii=False)+'\n'
 for ln in text.splitlines():lines+=textwrap.wrap(ln,width=116,replace_whitespace=False,drop_whitespace=False) or ['']
 im=Image.new('RGB',(1244,130+len(lines)*23),'#fbfbf8');d=ImageDraw.Draw(im);d.text((36,28),'Eunomia 2.2.0 gate 4: '+str(p.relative_to(s)),font=title,fill='#151815');d.text((36,72),'Full inputs and results are preserved in the adjacent JSONL and tools.json files.',font=font,fill='#505a55')
 for i,l in enumerate(lines):d.text((36,118+i*23),l,font=font,fill='#1d251f')
 out=s/'shots'/(str(p.relative_to(s)).replace('/','-').replace('.jsonl','-transcript.png'));im.save(out)
print('10 transcript PNGs generated from actual agent output and call evidence')
