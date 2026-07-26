import sys
from playwright.sync_api import sync_playwright
LAT, LON = 36.16048, -86.77843   # a real signalised junction
with sync_playwright() as p:
    b=p.chromium.launch(); pg=b.new_page(viewport={"width":1400,"height":900})
    errs=[]
    pg.on("pageerror", lambda e: errs.append("PAGEERROR "+str(e)[:220]))
    pg.on("console", lambda m: errs.append("console "+m.text[:180]) if m.type=="error" else None)
    pg.goto(f"http://127.0.0.1:8899/index.html#lat={LAT}&lon={LON}&zoom=18", wait_until="load", timeout=90000)
    pg.wait_for_function("() => !!window.__ptiles", timeout=30000)
    pg.wait_for_timeout(4000)
    # click dead centre of the map, which is the junction
    box = pg.query_selector("#map").bounding_box()
    pg.mouse.click(box["x"]+box["width"]/2, box["y"]+box["height"]/2)
    pg.wait_for_timeout(20000)
    vis = pg.evaluate("""() => {
      const s = document.getElementById('intxSection');
      const txt = id => (document.getElementById(id)||{}).textContent || '';
      const shownIds = ['bizRow','bizList','bldgGmapsRow','bldgCatRow'].filter(
        id => { const e=document.getElementById(id); return e && getComputedStyle(e).display!=='none'; });
      return { panelShown: document.getElementById('infoPanel').classList.contains('show'),
               intxShown: s && getComputedStyle(s).display!=='none',
               status: txt('infoStatus'), name: txt('intxName'), control: txt('intxControl'),
               dist: txt('intxDist'), approaches: txt('intxApproach'), osm: txt('intxOsmLink'),
               cams: txt('intxCam'),
               nodes: [...document.querySelectorAll('#intxNodes .row')].map(r=>r.innerText.replace(/\\s+/g,' ')),
               leakedBizOrBldgRows: shownIds };
    }""")
    for k,v in vis.items(): print(f"  {k}: {v}")
    pg.screenshot(path="/tmp/claude-1000/-home-aoi-kino/ec84e3e2-0a93-4c56-b16d-bdd348ef5e8d/scratchpad/intx.png")
    for e in list(dict.fromkeys(errs))[:6]: print("   "+e)
    b.close(); sys.exit(0 if vis["intxShown"] else 1)
