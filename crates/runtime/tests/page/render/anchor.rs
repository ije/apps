use super::*;

const PAGE: &str = "<head><style>body{margin:0}.s{width:160px;height:120px;overflow:auto}.g{width:400px;height:400px}\
     #a,#b{position:relative;left:70px;top:50px;width:40px;height:30px}#a{anchor-name:--a}#b{anchor-name:--b}\
     .o{position:relative;width:240px;height:180px}.x{position:absolute;width:24px;height:16px;position-anchor:--a}</style></head>\
     <body><div class=o><div class=s id=s><div class=g><div id=a></div></div></div>\
     <div class=x id=fn style='left:anchor(right);top:calc(anchor(bottom) + 2px)'></div>\
     <div class=x id=area style='position-area:right'></div>\
     <div class=x id=center style='justify-self:anchor-center;top:anchor(--no top, 5px);width:anchor-size(width)'></div>\
     <div class=x id=flip style='position-area:top;height:40px;position-try-fallbacks:flip-block'></div>\
     <div class=x id=rule style='position-area:left;width:100px;position-try-fallbacks:--r'></div></div>\
     <style>@position-try --r{position-area:none;left:anchor(right);top:anchor(top);width:30px}</style>\
     <button id=open>open</button><div popover id=pop style='margin:0;border:0;padding:0;width:20px;height:10px;position-area:bottom'></div><script>\
     for (const el of document.querySelectorAll('[id]')) globalThis[el.id] = el;\
     globalThis.at = el => { const r = el.getBoundingClientRect(); return [r.x, r.y, r.width, r.height].map(Math.round).join(' '); };\
     </script></body>";

#[test]
#[ignore = "requires the macOS window server"]
fn boxes_follow_their_anchors_like_chrome() {
  let text = run_hidden(
    r#"
        await load('anchor.html');
        const step = async (name, code) => console.log('ANCHOR ' + name + ' ' + await run(code));
        await step('fn', "at(fn)");
        await step('used', "getComputedStyle(fn).left + ' ' + getComputedStyle(fn).top");
        await step('area', "at(area)");
        await step('center', "at(center)");
        await step('flip', "at(flip)");
        await step('rule', "at(rule)");
        await step('moved', "a.style.top = '20px'; [at(fn), at(area), at(flip), getComputedStyle(flip).positionArea].join()");
        await step('kept', "a.style.top = '50px'; at(flip)");
        await step('fits', "a.style.top = '130px'; at(flip)");
        await step('back', "a.style.top = '50px'; flip.style.display = 'none'; at(fn); flip.style.display = ''; at(flip)");
        await step('width', "const wide = [s.scrollWidth, s.scrollHeight].join(); s.scrollTo(20, 30); wide");
        await win.webContents.capturePage();
        await step('scrolled', "[at(fn), at(area), [s.scrollWidth, s.scrollHeight]].join()");
        await step('layout', "getComputedStyle(fn).left");
        await run("s.scrollTo(0, 90)");
        await win.webContents.capturePage();
        await step('hidden', "const r = fn.getBoundingClientRect(); [document.elementFromPoint(r.x + 1, r.y + 1) === fn, getComputedStyle(fn).opacity].join()");
        await run("s.scrollTo(0, 0)");
        await win.webContents.capturePage();
        await step('shown', "const q = fn.getBoundingClientRect(); document.elementFromPoint(q.x + 1, q.y + 1) === fn");
        await step('popover', "open.style.cssText = 'position:absolute;left:100px;top:60px;width:50px;height:24px'; pop.showPopover({ source: open }); at(pop)");
        await step('none', "pop.hidePopover(); pop.showPopover(); at(pop)");
        await step('gone', "a.remove(); at(fn)");
        "#,
    &[("anchor.html", PAGE)],
  );
  for line in [
    "fn 110 82 24 16",
    "used 110px 82px",
    "area 110 57 24 16",
    "center 70 5 40 16",
    "flip 78 10 24 40",
    "rule 110 50 30 16",
    "moved 110 52 24 16,110 27 24 16,78 50 24 40,bottom",
    "kept 78 80 24 40",
    "fits 78 90 24 40",
    "back 78 10 24 40",
    "width 400,400",
    "scrolled 90 52 24 16,90 27 24 16,400,400",
    "layout 110px",
    "hidden false,1",
    "shown true",
    "popover 115 84 20 10",
  ] {
    assert!(text.contains(&format!("ANCHOR {line}\n")), "{line}: {text}");
  }
}

#[test]
#[ignore = "requires the macOS window server"]
fn position_try_rules_are_in_the_cssom() {
  run_hidden(
    r#"
        await load('rules.html');
        const read = code => run(`(() => { const rule = document.styleSheets[0].cssRules[0]; ${code} })()`);
        const first = await read("return [rule.constructor.name, rule.name, rule.cssText].join('|')");
        if (first !== 'CSSPositionTryRule|--p|@position-try --p { position-area: top; width: 5px; }') fail(first);
        const changed = await read("rule.style.color = 'red'; rule.style.width = '9px'; return rule.style.cssText + getComputedStyle(x).width");
        if (changed !== 'position-area: top; width: 9px;9px') fail(changed);
        "#,
    &[(
      "rules.html",
      "<style>@position-try --p{position-area:top;color:red;width:5px;height:1px!important}</style>\
             <body style='margin:0'><div style='position:absolute;left:100px;top:calc(100vh - 30px);width:20px;height:20px;anchor-name:--a'></div>\
             <div id=x style='position:absolute;position-anchor:--a;position-area:bottom;width:100px;height:90px;position-try-fallbacks:--p'></div>\
             <script>x = document.getElementById('x');</script></body>",
    )],
  );
}

#[test]
#[ignore = "requires the macOS window server"]
fn position_options_and_names_follow_chrome() {
  let text = run_hidden(
    r#"
        await load('options.html');
        const step = async (name, code) => console.log('OPTION ' + name + ' ' + await run(code));
        await step('own', "at(own)");
        await step('last', "at(last)");
        await step('limit', "at(limit)");
        await step('image', "at(image)");
        await step('ratio', "at(ratio)");
        await step('scope', "[...document.querySelectorAll('.in')].map(at).join()");
        await step('edit', "rules.sheet.cssRules[0].style.width = '30px'; at(edit)");
        await step('gone', "rules.sheet.deleteRule(0); at(edit)");
        await step('var', "v.style.setProperty('--w', '40px'); at(varied)");
        await step('vars', "v.style.setProperty('--w', '50px'); at(varied)");
        await step('none', "getComputedStyle(closed).left");
        "#,
    &[(
      "options.html",
      "<head><style id=rules>@position-try --e{position-area:right;width:50px;height:50px}</style>\
             <style>body{margin:0}.o{position:relative;width:240px;height:180px}\
             .a{position:relative;left:70px;top:50px;width:40px;height:30px;anchor-name:--a}\
             .x{position:absolute;position-anchor:--a}.r{anchor-scope:--r;position:relative;height:20px}\
             @position-try --b{position-area:top;width:300px;height:400px}@position-try --ok{position-area:right;width:20px;height:20px}\
             @position-try --v{position-area:right;width:var(--w);height:20px}</style></head><body>\
             <div class=o><div class=a></div><div class=x id=own style='position-area:top;width:200px;height:150px;position-try:most-height flip-block,right'></div></div>\
             <div class=o><div class=a></div><div class=x id=last style='position-area:top;width:500px;height:500px;position-try-fallbacks:--gone,--b,--b,--b,--b,--ok'></div></div>\
             <div class=o><div class=a></div><div class=x id=limit style='position-area:top;width:500px;height:500px;position-try-fallbacks:--b,--b,--b,--b,--b,--ok'></div></div>\
             <div class=o><div class=a></div><img class=x id=image style='position-area:bottom;height:40px;aspect-ratio:3/1'>\
             <div class=x id=ratio style='position-area:bottom;height:30px;aspect-ratio:2'></div></div>\
             <div class=o><div class=a></div><div class=x id=edit style='position-area:bottom;width:100px;height:400px;position-try-fallbacks:--e'></div></div>\
             <div class=o id=v><div class=a></div><div class=x id=varied style='position-area:bottom;width:100px;height:400px;position-try-fallbacks:--v'></div></div>\
             <div class=x id=closed style='display:none;left:anchor(left, 9px)'></div>\
             <div class=r><b style='anchor-name:--r;display:block;width:30px'>1</b><div class='x in' style='position-anchor:--r;left:anchor(right);top:0;width:5px;height:5px'></div></div>\
             <div class=r><b style='anchor-name:--r;display:block;width:60px'>2</b><div class='x in' style='position-anchor:--r;left:anchor(right);top:0;width:5px;height:5px'></div></div>\
             <script>for (const el of document.querySelectorAll('[id]')) globalThis[el.id] = el;\
             globalThis.at = el => { const r = el.getBoundingClientRect(), o = el.offsetParent.getBoundingClientRect(); return [r.x - o.x, r.y - o.y, r.width, r.height].map(Math.round).join(' '); };\
             </script></body>",
    )],
  );
  for line in [
    "own 0 0 200 150",
    "last 110 55 20 20",
    "limit 0 0 500 500",
    "image 30 80 120 40",
    "ratio 60 80 60 30",
    "scope 30 0 5 5,60 0 5 5",
    "edit 110 40 30 50",
    "gone 40 0 100 400",
    "var 110 55 40 20",
    "vars 110 55 50 20",
    "none 9px",
  ] {
    assert!(text.contains(&format!("OPTION {line}\n")), "{line}: {text}");
  }
}

#[test]
#[ignore = "requires the macOS window server"]
fn late_boxes_and_new_scrollers_follow_chrome() {
  let text = run_hidden(
    r#"
        await load('late.html');
        const step = async (name, code) => { await run(code); await win.webContents.capturePage(); console.log('LATE ' + name + ' ' + await run("[at(late), at(box)].join()")); };
        await step('hidden', "0");
        await step('shown', "wrap.style.display = 'block'");
        await step('block', "wrap.style.position = 'relative'");
        await step('static', "wrap.style.position = ''");
        await step('scope', "wrap.style.anchorScope = '--a'");
        await step('free', "wrap.style.anchorScope = ''");
        await step('transform', "wrap.style.transform = 'translate(0)'");
        await step('plain', "wrap.style.transform = ''");
        await step('scrolled', "s.scrollTop = 40");
        await step('spilled', "s.style.overflow = 'visible'");
        await step('moved', "a.style.left = '20px'");
        console.log('LATE closed ' + await run("late.style.display = 'none'; getComputedStyle(late).width"));
        await step('reopened', "late.style.display = 'block'");
        console.log('LATE fixed ' + await run("document.elementFromPoint(205, 155)?.id"));
        await step('clip', "a.style.left = ''; s.style.overflow = ''; s.scrollTop = 200");
        console.log('LATE clipped ' + await run("document.elementFromPoint(205, 155)?.id"));
        "#,
    &[(
      "late.html",
      "<body style='margin:0'><style>#o{position:relative;width:240px;height:400px}#s{width:160px;height:120px;overflow:auto}#g{height:400px}\
             #a{position:relative;left:70px;top:50px;width:40px;height:30px;anchor-name:--a}\
             .x{position:absolute;position-anchor:--a;position-area:bottom right;width:20px;height:10px}</style>\
             <div id=o><div id=s><div id=g><div id=a></div></div></div>\
             <div id=wrap style='display:none;padding:7px 9px'><div class=x id=late style='width:anchor-size(width, 20px)'></div></div>\
             <div class=x id=box style='position-area:bottom'><i id=deep style='position:fixed;left:200px;top:150px;width:10px;height:10px'></i></div></div>\
             <script>for (const el of document.querySelectorAll('[id]')) globalThis[el.id] = el;\
             globalThis.at = el => { const r = el.getBoundingClientRect(); return [r.x, r.y, r.width, r.height].map(Math.round).join(' '); };\
             </script></body>",
    )],
  );
  for line in [
    "hidden 0 0 0 0,80 80 20 10",
    "shown 110 80 40 10,80 80 20 10",
    "block 9 127 20 10,80 80 20 10",
    "scope 9 127 20 10,80 80 20 10",
    "transform 9 127 20 10,80 80 20 10",
    "static 110 80 40 10,80 80 20 10",
    "free 110 80 40 10,80 80 20 10",
    "plain 110 80 40 10,80 80 20 10",
    "scrolled 110 40 40 10,80 40 20 10",
    "spilled 110 80 40 10,80 80 20 10",
    "moved 60 80 40 10,30 80 20 10",
    "closed 20px",
    "reopened 60 80 40 10,30 80 20 10",
    "fixed deep",
  ] {
    assert!(text.contains(&format!("LATE {line}\n")), "{line}: {text}");
  }
  assert!(!text.contains("LATE clipped deep"), "{text}");
}
