"""Drag-and-drop launcher: `csvdiff serve` then open http://127.0.0.1:8765.

Drop two CSVs, type the key (or pick a profile), get the report in a new tab.
Uses only the standard library so it runs anywhere Python does.
"""
from __future__ import annotations

import email
import email.policy
import json
import os
import tempfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from .config import load_config, options_from, parse_list
from .engine import CompareError, compare
from .report import render

PAGE = r"""<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>csvdiff</title>
<style>
:root{--ink:#1c2026;--mute:#6a7180;--rule:#e3e6ea;--bg:#fff;--soft:#f5f6f8;--focus:#2f5bd8;--add:#237a48;--rem:#c0392b}
@media (prefers-color-scheme:dark){:root{--ink:#e6e8ec;--mute:#9aa1ad;--rule:#2b3038;--bg:#15181c;--soft:#1d2126}}
*{box-sizing:border-box}body{margin:0;font:15px/1.5 "Segoe UI",system-ui,sans-serif;color:var(--ink);background:var(--bg)}
main{max-width:720px;margin:0 auto;padding:40px 20px}h1{font-size:22px;font-weight:600;margin:0 0 4px}.sub{color:var(--mute);margin:0 0 28px}
.drops{display:grid;grid-template-columns:1fr 1fr;gap:14px}
.drop{border:2px dashed var(--rule);border-radius:10px;padding:28px 16px;text-align:center;color:var(--mute);cursor:pointer;min-height:120px;display:flex;flex-direction:column;justify-content:center}
.drop.over{border-color:var(--focus);background:var(--soft)}.drop.ok{border-style:solid;color:var(--ink)}.drop b{display:block;font-size:13px;margin-bottom:6px;color:var(--mute)}
.drop input{display:none}
label.f{display:block;margin-top:18px}label.f span{display:block;color:var(--mute);font-size:13px;margin-bottom:4px}
input[type=text],select{width:100%;padding:8px 10px;border:1px solid var(--rule);border-radius:6px;background:var(--bg);color:inherit;font:inherit}
.opts{display:flex;flex-wrap:wrap;gap:8px 20px;margin-top:14px;color:var(--mute)}.opts label{display:flex;gap:6px;align-items:center}
.opts input[type=number]{width:90px;padding:4px 6px;border:1px solid var(--rule);border-radius:4px;background:var(--bg);color:inherit}
button.go{margin-top:24px;padding:10px 22px;border:0;border-radius:6px;background:var(--focus);color:#fff;font:inherit;font-weight:600;cursor:pointer}
button.go:disabled{opacity:.5;cursor:default}#msg{margin-top:14px;color:var(--rem);min-height:1.5em}#msg.ok{color:var(--add)}
</style></head><body><main>
<h1>Compare two CSV files</h1><p class="sub">Drop the files, name the key columns, and the report opens in a new tab.</p>
<div class="drops">
 <label class="drop" id="da"><b>File A (before)</b><span>Drop or click</span><input type="file" accept=".csv,.txt,.tsv"></label>
 <label class="drop" id="db"><b>File B (after)</b><span>Drop or click</span><input type="file" accept=".csv,.txt,.tsv"></label>
</div>
<label class="f"><span>Profile</span><select id="profile"><option value="">None, use the fields below</option>__PROFILES__</select></label>
<label class="f"><span>Key columns, comma separated</span><input type="text" id="key" placeholder="order_id, line_no" autocomplete="off"></label>
<label class="f"><span>Columns to compare (blank = every common column that is not a key)</span><input type="text" id="compare" autocomplete="off"></label>
<label class="f"><span>Columns to ignore</span><input type="text" id="ignore" autocomplete="off"></label>
<div class="opts">
 <label><input type="checkbox" id="trim">Trim whitespace</label>
 <label><input type="checkbox" id="ic">Ignore case</label>
 <label><input type="checkbox" id="en">Empty equals null</label>
 <label>Numeric tolerance <input type="number" id="tol" step="any" min="0" value="0"></label>
</div>
<button class="go" id="go" disabled>Compare</button><div id="msg"></div>
</main>
<script>
const F={a:null,b:null};
for(const id of ['da','db']){const z=document.getElementById(id),inp=z.querySelector('input'),k=id[1];
 const set=f=>{F[k]=f;z.classList.toggle('ok',!!f);z.querySelector('span').textContent=f?f.name+' ('+(f.size/1048576).toFixed(1)+' MB)':'Drop or click';check();};
 inp.onchange=()=>set(inp.files[0]);
 z.ondragover=e=>{e.preventDefault();z.classList.add('over')};z.ondragleave=()=>z.classList.remove('over');
 z.ondrop=e=>{e.preventDefault();z.classList.remove('over');set(e.dataTransfer.files[0]);};}
const key=document.getElementById('key'),prof=document.getElementById('profile'),go=document.getElementById('go'),msg=document.getElementById('msg');
function check(){go.disabled=!(F.a&&F.b&&(key.value.trim()||prof.value));}
key.oninput=check;prof.onchange=check;
document.addEventListener('paste',e=>{const fs=[...(e.clipboardData.files||[])];if(fs.length>=2){document.querySelector('#da input').files=e.clipboardData.files;}});
go.onclick=async()=>{go.disabled=true;msg.className='';msg.textContent='Comparing…';
 const fd=new FormData();fd.append('a',F.a);fd.append('b',F.b);
 for(const id of ['key','compare','ignore','profile'])fd.append(id,document.getElementById(id).value);
 fd.append('trim',document.getElementById('trim').checked);fd.append('ignore_case',document.getElementById('ic').checked);
 fd.append('empty_is_null',document.getElementById('en').checked);fd.append('tolerance',document.getElementById('tol').value);
 try{const r=await fetch('/compare',{method:'POST',body:fd});
  if(!r.ok){msg.textContent=await r.text();go.disabled=false;return;}
  const html=await r.text();const s=r.headers.get('X-Summary')||'';
  const u=URL.createObjectURL(new Blob([html],{type:'text/html'}));window.open(u,'_blank');
  const a=document.createElement('a');a.href=u;a.download=(F.a.name.replace(/\.[^.]+$/,'')+'__vs__'+F.b.name.replace(/\.[^.]+$/,'')+'.html');a.textContent='Save report';
  msg.className='ok';msg.textContent=s+' ';msg.appendChild(a);
 }catch(e){msg.textContent='Request failed: '+e;}
 go.disabled=false;};
</script></body></html>"""


class Handler(BaseHTTPRequestHandler):
    config: dict = {}

    def log_message(self, fmt, *args):  # quieter
        if "/compare" in (args[0] if args else ""):
            super().log_message(fmt, *args)

    def _send(self, code: int, body: bytes, ctype="text/html; charset=utf-8", headers=None):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        profiles = self.config.get("profiles", {})
        opts = "".join(f'<option value="{n}">{n}: key {", ".join(p.get("key", []))}</option>' for n, p in profiles.items())
        self._send(200, PAGE.replace("__PROFILES__", opts).encode("utf-8"))

    def do_POST(self):
        if self.path != "/compare":
            return self._send(404, b"not found", "text/plain")
        length = int(self.headers.get("Content-Length", 0))
        raw = b"Content-Type: " + self.headers["Content-Type"].encode() + b"\r\n\r\n" + self.rfile.read(length)
        msg = email.message_from_bytes(raw, policy=email.policy.HTTP)
        fields, files = {}, {}
        for part in msg.iter_parts():
            name = part.get_param("name", header="content-disposition")
            if part.get_filename():
                files[name] = (part.get_filename(), part.get_payload(decode=True))
            else:
                fields[name] = part.get_payload(decode=True).decode("utf-8", "replace")
        if "a" not in files or "b" not in files:
            return self._send(400, b"Two files are required.", "text/plain")
        with tempfile.TemporaryDirectory() as td:
            pa, pb = os.path.join(td, "a_" + files["a"][0]), os.path.join(td, "b_" + files["b"][0])
            open(pa, "wb").write(files["a"][1]), open(pb, "wb").write(files["b"][1])
            profile = self.config.get("profiles", {}).get(fields.get("profile") or "")
            try:
                opt = options_from(profile, {
                    "key": parse_list(fields.get("key")) or None, "compare": parse_list(fields.get("compare")) or None,
                    "ignore": parse_list(fields.get("ignore")) or None,
                    "trim": fields.get("trim") == "true" or None, "ignore_case": fields.get("ignore_case") == "true" or None,
                    "empty_is_null": fields.get("empty_is_null") == "true" or None,
                    "tolerance": float(fields.get("tolerance") or 0) or None,
                })
                if not opt.key:
                    raise CompareError("Key columns are required.")
                result = compare(pa, pb, opt)
            except (CompareError, ValueError) as e:
                return self._send(400, str(e).encode(), "text/plain; charset=utf-8")
            c = result["counts"]
            summary = (f"Matched {c['matched']:,} (changed {c['changed']:,}), added {c['added']:,}, "
                       f"removed {c['removed']:,}, {result['meta']['seconds']}s.")
            self._send(200, render(result).encode("utf-8"), headers={"X-Summary": summary})


def serve(host="127.0.0.1", port=8765, config_path=None):
    Handler.config = load_config(config_path)
    srv = ThreadingHTTPServer((host, port), Handler)
    print(f"csvdiff drop page: http://{host}:{port}   (Ctrl+C to stop)")
    srv.serve_forever()
