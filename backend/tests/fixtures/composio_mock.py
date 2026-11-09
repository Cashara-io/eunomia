#!/usr/bin/env python3
"""Consumer MCP fixture. No real account, credentials, network calls or app data."""
import base64
import json
import hashlib
import threading
from pathlib import Path
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs, urlencode

TOKEN = 'composio-canary-access'
REFRESH = 'composio-canary-refresh'
state = {'limited': False, 'refreshes': 0, 'revokes': 0, 'executions': [], 'registered': [], 'schema_requests': [], 'challenge': '', 'refresh_token': REFRESH, 'revoke_refresh': False, 'pause_refresh': False, 'refresh_started': False}
refresh_resume = threading.Event()
APPS = ['googledrive', 'gmail', 'googlecalendar', 'timelinesai', 'whatsapp', 'github']
class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    @property
    def origin(self): return f'http://127.0.0.1:{self.server.server_port}'
    def send(self, value, status=200, headers=None):
        payload = json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(payload)))
        for k,v in (headers or {}).items(): self.send_header(k, v)
        self.end_headers(); self.wfile.write(payload)
    def do_GET(self):
        path = urlparse(self.path).path
        if path == '/.well-known/oauth-protected-resource/mcp': return self.send({'resource':self.origin+'/mcp','authorization_servers':[self.origin]})
        if path == '/.well-known/oauth-authorization-server': return self.send({'issuer':self.origin,'authorization_endpoint':self.origin+'/authorize','token_endpoint':self.origin+'/token','registration_endpoint':self.origin+'/register','revocation_endpoint':self.origin+'/revoke','code_challenge_methods_supported':['S256'],'token_endpoint_auth_methods_supported':['none']})
        if path == '/.well-known/openid-configuration': return self.send({'issuer':self.origin,'userinfo_endpoint':self.origin+'/userinfo'})
        if path == '/userinfo':
            assert self.headers.get('Authorization') == 'Bearer '+TOKEN
            return self.send({'email':'mock-person@example.test'})
        if path == '/authorize':
            q = parse_qs(urlparse(self.path).query)
            assert q['code_challenge_method'] == ['S256'] and len(q['code_challenge'][0]) == 43
            state['challenge'] = q['code_challenge'][0]
            state['code_used'] = False
            self.send_response(302); self.send_header('Location',q['redirect_uri'][0]+'?'+urlencode({'code':'one-use-mock-code','state':q['state'][0]})); self.end_headers(); return
        if path.startswith('/file'):
            assert not self.headers.get('Authorization') and not self.headers.get('Mcp-Session-Id')
            content = b'Drive searchable fixture content.\n'
            if path in ['/file/pdf', '/file/sniff']: content = Path(__file__).with_name('notes.pdf').read_bytes()
            if path == '/file/corrupt': content = b'%PDF-invalid'
            if path == '/file/invalid': content = b'\xff\xfe'
            self.send_response(200);self.send_header('Content-Type','text/plain');self.send_header('Content-Length',str(len(content)));self.end_headers();self.wfile.write(content);return
        if path == '/stats': return self.send(state)
        self.send({'error':'missing'},404)
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get('Content-Length','0')))
        if self.path == '/control':
            state.update(json.loads(body))
            if not state['pause_refresh']: refresh_resume.set()
            else: refresh_resume.clear()
            return self.send({})
        if self.path == '/register':
            registration = json.loads(body); assert registration['token_endpoint_auth_method']=='none'
            state['registered'].append(registration)
            return self.send({'client_id':'mock-public-client'})
        if self.path == '/token':
            form = parse_qs(body.decode())
            if form['grant_type']==['authorization_code']:
                assert form['code']==['one-use-mock-code']
                challenge = base64.urlsafe_b64encode(hashlib.sha256(form['code_verifier'][0].encode()).digest()).rstrip(b'=').decode()
                if challenge != state['challenge'] or state['code_used']: return self.send({'error':'invalid_grant'},400)
                state['code_used'] = True
                state['limited']=False
            else:
                state['refresh_started'] = True
                if state['pause_refresh']: assert refresh_resume.wait(10), 'refresh was not released'
                if state['revoke_refresh'] or form['refresh_token'] != [state['refresh_token']]:
                    return self.send({'error':'invalid_grant'},400)
                state['refreshes'] += 1
                state['refresh_token'] = REFRESH + '-' + str(state['refreshes'])
            return self.send({'access_token':TOKEN,'refresh_token':state['refresh_token'],'expires_in':1 if form['grant_type']==['authorization_code'] else 3600,'token_type':'Bearer'})
        if self.path == '/revoke': state['revokes']+=1; return self.send({})
        if self.path != '/mcp': return self.send({},404)
        assert self.headers.get('Authorization') == 'Bearer '+TOKEN
        request = json.loads(body); method=request['method']; rid=request.get('id')
        def rpc(value): self.send({'jsonrpc':'2.0','id':rid,'result':value},headers={'Mcp-Session-Id':'mock-session-canary'})
        if method == 'initialize': return rpc({'protocolVersion':'2025-03-26','capabilities':{'tools':{}},'serverInfo':{'name':'mock','version':'1'}})
        if method == 'notifications/initialized': return self.send({},202)
        assert self.headers.get('Mcp-Session-Id') == 'mock-session-canary'
        if method == 'tools/list': return rpc({'tools':[]})
        name=request['params']['name']; args=request['params']['arguments']
        def tool(value): rpc({'content':[{'type':'text','text':json.dumps(value)}]})
        if name == 'COMPOSIO_SEARCH_TOOLS':
            use = args['queries'][0]['use_case']; slugs = [word for word in use.split() if word.startswith(('GOOGLE','GMAIL_','TIMELINESAI_','WHATSAPP_','GITHUB_'))]
            return tool({'successful':True,'data':{'session':{'id':'mock-search'},'tool_slugs':slugs}})
        if name == 'COMPOSIO_GET_TOOL_SCHEMAS':
            state['schema_requests'].extend(args['tool_slugs'])
            return tool({'successful':True,'data':{'tool_schemas':{slug:{'input_schema':{'type':'object'}} for slug in args['tool_slugs']}}})
        if name == 'COMPOSIO_MANAGE_CONNECTIONS':
            app=args['toolkits'][0]['name']
            if args['toolkits'][0]['action']=='add': return tool({'successful':True,'data':{'redirect_url':self.origin+'/authorise-app'}})
            accounts=[{'id':app+'-mine','alias':'Fixture '+app,'status':'ACTIVE'}]
            if app=='googledrive': accounts.append({'id':'googledrive-second','alias':'Second Drive','status':'ACTIVE'})
            return tool({'successful':True,'data':{'accounts':accounts}})
        assert name == 'COMPOSIO_MULTI_EXECUTE_TOOL'
        call=args['tools'][0]; slug=call['tool_slug']; a=call['arguments']; account=call['account']
        assert account in [slug.split('_')[0].lower()+'-mine','googledrive-second']
        state['executions'].append({'slug':slug,'account':account})
        def output(data): tool({'successful':True,'data':{'results':[{'tool_slug':slug,'response':{'successful':True,'data':data}}]}})
        if slug=='GMAIL_GET_PROFILE': return output({'historyId':'100'})
        if slug=='GMAIL_LIST_HISTORY': return output({'history':[],'historyId':'101'})
        if slug=='GMAIL_FETCH_EMAILS': assert a['ids_only']; return output({'messages':[{'messageId':'m2'},{'messageId':'m1'}],'nextPageToken':''})
        if slug=='GMAIL_FETCH_MESSAGE_BY_MESSAGE_ID':
            if a['message_id']=='m2' and not state['limited']:
                state['limited']=True
                return tool({'successful':True,'data':{'results':[{'response':{'successful':False,'error':{'status_code':429,'message':TOKEN}}}]}})
            return output({'messageId':a['message_id'],'threadId':'t1','messageTimestamp':'2026-11-09T00:00:00Z','subject':'Fixture email '+a['message_id'],'sender':'friend@example.test','to':'mock@example.test','messageText':'Searchable email content '+TOKEN})
        if slug=='GOOGLEDRIVE_FIND_FILE':
            files = [{'id':'doc1','name':'Drive fixture','mimeType':'application/vnd.google-apps.document'}]
            for fid, filename in [('pdf','fallback.pdf'), ('sniff','sniff.txt'), ('corrupt','corrupt.pdf'), ('invalid','invalid.txt'), ('tail','tail.txt')]:
                files.append({'id':fid,'name':filename,'mimeType':'application/octet-stream'})
            for file in files: file.update({'modifiedTime':'2026-11-08T00:00:00Z','webViewLink':'https://drive.google.com/file/d/'+file['id']})
            return output({'files':files,'nextPageToken':'','incompleteSearch':False})
        if slug=='GOOGLEDRIVE_DOWNLOAD_FILE': return output({'downloaded_file_content':{'s3url':self.origin+'/file/'+a['fileId']}})
        if slug=='GOOGLEDRIVE_EXPORT_GOOGLE_WORKSPACE_FILE': return output({'file':{'name':'fixture.txt','mimetype':'text/plain','s3url':self.origin+'/file'}})
        if slug=='GOOGLECALENDAR_EVENTS_LIST': return output({'items':[{'id':'event1','summary':'Fixture meeting','description':'Calendar searchable content','start':{'dateTime':'2026-11-09T02:00:00Z'},'end':{'dateTime':'2026-11-09T03:00:00Z'},'status':'confirmed'}],'nextSyncToken':'calendar-sync1'})
        if slug=='TIMELINESAI_GET_CHATS': return output({'results':[{'id':'chat1'}],'has_more_pages':False})
        if slug=='TIMELINESAI_GET_CHAT_MESSAGES': return output({'messages':[{'id':'message1','text':'Personal WhatsApp fixture content','timestamp':'2026-11-09T00:00:00Z'}],'count':1})
        if slug=='WHATSAPP_GET_MESSAGE_HISTORY': return output({'data':[{'id':'delivery1','message_id':'business1','events':[{'delivery_status':'DELIVERED'}]}]})
        if slug=='GITHUB_GET_A_TREE': return output({'tree':[{'type':'blob','path':'Note.md'}],'sha':'fixture-commit','truncated':False})
        if slug=='GITHUB_GET_REPOSITORY_CONTENT': return output({'content':{'content':base64.b64encode(b'---\nparent: "[[Other]]"\ntags: [project]\n---\nObsidian searchable fixture. [[Other]]').decode(),'encoding':'base64','html_url':'https://github.com/fixture/vault/blob/main/Note.md'}})
        raise AssertionError(slug)
server=ThreadingHTTPServer(('127.0.0.1',int(sys.argv[1])),Handler)
server.serve_forever()
