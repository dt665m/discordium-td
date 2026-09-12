"""Build consistent HTML reports and embed canonical subsystem/protocol sources.

Requires mistune. All generated pages are self-contained except source/package
links. Run from any directory: python /path/to/package/build_report.py
"""
from pathlib import Path
import html
import json
import re
import mistune

P = Path(__file__).resolve().parent
SOURCE_REF = re.compile(r'\[S(\d{2})(?:[–-]S(\d{2}))?\](?:\((?:ENGINEERING_SPEC\.(?:md|html))?#s\d{2}\))?')

def link_sources(text, target=''):
    def ref(m):
        first, last = int(m[1]), int(m[2] or m[1])
        return ' '.join(f'[S{i:02d}]({target}#s{i:02d})' for i in range(first,last+1))
    return SOURCE_REF.sub(ref,text)

def embedded(text, section, title):
    lines = text.splitlines()
    assert lines[0].startswith('# ')
    lines[0] = f'## {section}. {title}'
    for i,line in enumerate(lines[1:],1):
        lines[i] = re.sub(r'^## (\d+)\. ',lambda m:f'### {section}.{m[1]}. ',line)
    return link_sources('\n'.join(lines)).rstrip()+'\n\n'

def replace_section(text, n, replacement):
    start = text.index(f'## {n}. ')
    end = text.index(f'## {n+1}. ',start) if n < 27 else text.index('## Sources',start)
    return text[:start]+replacement+text[end:]

CSS = '''
:root{--ink:#1d1f22;--muted:#5d6269;--line:#dfe2e5;--panel:#f5f6f7;--link:#244b69}
*{box-sizing:border-box}html{scroll-behavior:smooth;scroll-padding-top:24px}
body{margin:0;background:#fff;color:var(--ink);font:16px/1.65 system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}
nav{position:fixed;inset:0 auto 0 0;width:276px;padding:28px 20px 32px;overflow:auto;background:var(--panel);border-right:1px solid var(--line)}
nav .label{font-size:15px;font-weight:650;margin-bottom:16px}nav a{display:block;font-size:12px;line-height:1.45;color:#42474e;text-decoration:none;padding:6px 0}nav a:hover{text-decoration:underline;color:#000}
nav .jump{font-weight:650;padding-bottom:12px;margin-bottom:12px;border-bottom:1px solid var(--line)}
main{max-width:1190px;margin-left:276px;padding:44px 52px 90px}h1{font-size:38px;line-height:1.2;letter-spacing:-.8px;margin:0 0 30px;font-weight:700}h2{font-size:27px;line-height:1.3;margin:58px 0 22px;letter-spacing:-.4px;border-top:1px solid var(--line);padding-top:28px}h3{font-size:20px;line-height:1.4;margin:32px 0 12px}
p{margin:12px 0 18px}a{color:var(--link);text-underline-offset:3px}a.ref{font-size:12px;vertical-align:super;text-decoration:none;white-space:nowrap;margin-right:2px}strong{font-weight:650}
.table-scroll{max-width:100%;overflow-x:auto;margin:22px 0 28px}table{border-collapse:collapse;width:100%;font-size:13px;line-height:1.55}th{text-align:left;background:#f1f2f3;font-weight:650;border-top:1px solid #c6cbd0}th,td{padding:12px;vertical-align:top;border-bottom:1px solid var(--line);min-width:105px}td:first-child{font-weight:550}tr:nth-child(even){background:#fafafa}
pre{padding:20px 22px;background:#f4f5f6;border:1px solid var(--line);overflow-x:auto;font:12px/1.65 ui-monospace,SFMono-Regular,Consolas,monospace}code{font-family:ui-monospace,SFMono-Regular,Consolas,monospace;font-size:.87em}p code,td code,li code{background:#f1f2f3;padding:2px 4px}pre code{font-size:inherit;background:none;padding:0}ul,ol{padding-left:24px}li{margin-bottom:9px}a[id]{scroll-margin-top:30px}
@media(min-width:1550px){main{margin-left:calc(276px + (100vw - 1550px)/2)}}
@media(max-width:1000px){nav{width:230px}main{margin-left:230px;padding:32px 28px}h1{font-size:32px}table{font-size:12px}}
@media(max-width:720px){nav{position:relative;width:auto;max-height:250px;border-right:0;border-bottom:1px solid var(--line);padding:18px 24px}nav a{font-size:12px;padding:4px 0}main{margin:0;padding:30px 20px 60px}h1{font-size:30px}h2{font-size:23px}body{font-size:15px}.table-scroll table{min-width:640px}}
@media print{nav{display:none}main{margin:0;max-width:none;padding:0}body{font-size:10pt;line-height:1.45}h1{font-size:24pt}h2{font-size:17pt;break-after:avoid;margin-top:28px}h3{font-size:13pt;break-after:avoid}pre{white-space:pre-wrap;overflow:visible;font-size:8pt}.table-scroll{overflow:visible}table{font-size:8pt}tr{break-inside:avoid}a{color:#222;text-decoration:none}a.ref{font-size:7pt}p{orphans:3;widows:3}}
'''

def render(text,title,main=False):
    md = mistune.create_markdown(escape=False,plugins=['table','strikethrough'])
    body = md(text)
    nav, ids = [], set()
    def heading(m):
        level, content = m[1],m[2]
        plain = html.unescape(re.sub('<[^>]+>','',content))
        slug = re.sub('[^a-z0-9]+','-',plain.lower()).strip('-')
        if slug in ids: raise ValueError('duplicate heading '+slug)
        ids.add(slug)
        if level == '2': nav.append(f'<a href="#{slug}">{content}</a>')
        return f'<h{level} id="{slug}">{content}</h{level}>'
    body = re.sub(r'<h([23])>(.*?)</h\1>',heading,body)
    body = body.replace('<table>','<div class="table-scroll"><table>').replace('</table>','</table></div>')
    body = re.sub(r'<a href="([^\"]*#s\d\d)">',r'<a class="ref" href="\1">',body)
    if main:
        jump='<a class="jump" href="#17-fortnite-unreal-replication-graph-spatial-foundation">Spatial replication foundation</a>'
    else:
        jump='<a class="jump" href="ENGINEERING_SPEC.html">Complete engineering specification</a>'
    return ('<!doctype html><html lang="en"><head><meta charset="utf-8">'
            '<meta name="viewport" content="width=device-width,initial-scale=1">'
            f'<title>{html.escape(title)}</title><style>{CSS}</style></head><body>'
            '<nav aria-label="Contents"><div class="label">Contents</div>'+jump+''.join(nav)
            +'</nav><main>'+body+'</main></body></html>')

def build():
    sources=json.loads((P/'sources.json').read_text(encoding='utf-8'))
    spatial=link_sources((P/'SPATIAL_REPLICATION.md').read_text(encoding='utf-8'),'ENGINEERING_SPEC.md')
    protocol=link_sources((P/'PROTOCOL_ADDENDUM.md').read_text(encoding='utf-8'),'ENGINEERING_SPEC.md')
    (P/'SPATIAL_REPLICATION.md').write_text(spatial,encoding='utf-8')
    (P/'PROTOCOL_ADDENDUM.md').write_text(protocol,encoding='utf-8')
    text=(P/'ENGINEERING_SPEC.md').read_text(encoding='utf-8')
    text=replace_section(text,17,embedded(spatial,17,'Fortnite / Unreal Replication Graph spatial foundation'))
    text=replace_section(text,27,embedded(protocol,27,'Protocol completion contracts'))
    text=text[:text.index('## Sources')]+ '## Sources\n\n'
    for s in sources:
        text+=f'<a id="{s["id"].lower()}"></a>\n\n**{s["id"]}. {s["publisher"]}. [{s["title"]}]({s["url"]}).** {s["version"]}.\n\nEvidence scope: {s["scope"]}\n\n'
        if s.get('evidence_role'):text+=f'Architecture role: {s["evidence_role"]}.\n\n'
    text=link_sources(text)
    (P/'ENGINEERING_SPEC.md').write_text(text,encoding='utf-8')
    (P/'ENGINEERING_SPEC.html').write_text(render(text,'Server-Authoritative Prediction Library',True),encoding='utf-8')
    for filename,content,title in [('SPATIAL_REPLICATION',spatial,'Fortnite / Unreal Replication Graph Spatial Foundation'),
                                    ('PROTOCOL_ADDENDUM',protocol,'Protocol Completion Contracts')]:
        (P/(filename+'.html')).write_text(render(link_sources(content,'ENGINEERING_SPEC.html'),title),encoding='utf-8')
    print(f'Rendered main specification and two focused reports; {len(sources)} source entries')

if __name__=='__main__':
    build()
