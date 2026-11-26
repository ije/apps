use crate::css::{properties::Prop, sheet::Stylesheet};
use std::rc::Rc;
pub(crate) const UA: &str = "
html,body,address,blockquote,center,dialog,div,figure,figcaption,footer,form,header,hr,legend,main,p,pre,search,article,aside,h1,h2,h3,h4,h5,h6,hgroup,nav,section,dd,dl,dt,menu,dir,ol,ul,details,summary,fieldset,optgroup,option{display:block}
li{display:list-item}
img,svg,canvas,video,audio,iframe,embed,object,input,button,select,textarea,progress,meter{display:inline-block}
input:not([type=hidden i],[type=image i],[type=file i]),button,select,textarea,meter,progress{appearance:auto}
canvas[content=drawable i]{-canvas-content:drawable}
canvas[content=drawable i] [drawable]{isolation:isolate;contain:layout}
head,script,style,title,meta,link,template,noscript,datalist,base,param,[hidden]:not([hidden=until-found i]),input[type=hidden],dialog:not([open]){display:none}
[hidden=until-found i]{content-visibility:hidden}
audio:not([controls]),embed:not([src]):not([type]){display:none!important}
audio{width:300px;height:54px}
video{object-fit:contain}
terminal{display:inline-block;width:80ch;height:24lh;overflow:hidden;font:13px ui-monospace,Menlo,Consolas,monospace;cursor:text;user-select:none}
slot{display:contents}
[dir]{unicode-bidi:isolate}
bdi,output{unicode-bidi:isolate}
bdo,bdo[dir]{unicode-bidi:isolate-override}
[dir=rtl i]{direction:rtl}
[dir=ltr i],[dir=auto i]{direction:ltr}
bdi{direction:ltr}
[dir=auto i]:dir(rtl),bdi:dir(rtl){direction:rtl}
textarea[dir=auto i],pre[dir=auto i]{unicode-bidi:plaintext}
body{margin:8px}
p,dl,ul,ol,menu,dir,pre{margin:1em 0}
blockquote,figure{margin:1em 40px}
ul ul,ul ol,ol ul,ol ol{margin:0}
ul,ol,menu,dir{padding-inline-start:40px}
ul,menu,dir{list-style-type:disc}
ol{list-style-type:decimal}
ul ul,ol ul{list-style-type:circle}
ul ul ul,ol ul ul,ul ol ul,ol ol ul{list-style-type:square}
dd{margin-inline-start:40px}
h1{font-size:2em;margin:.67em 0;font-weight:bold}
h2{font-size:1.5em;margin:.83em 0;font-weight:bold}
h3{font-size:1.17em;margin:1em 0;font-weight:bold}
h4{margin:1.33em 0;font-weight:bold}
h5{font-size:.83em;margin:1.67em 0;font-weight:bold}
h6{font-size:.67em;margin:2.33em 0;font-weight:bold}
b,strong,th{font-weight:bolder}
i,em,cite,var,dfn,address{font-style:italic}
pre,code,kbd,samp,tt{font-family:monospace}
pre,textarea{white-space:pre-wrap}
pre{white-space:pre}
small,sub,sup{font-size:smaller}
sub{vertical-align:sub}
sup{vertical-align:super}
big{font-size:larger}
u,ins{text-decoration:underline}
s,strike,del{text-decoration:line-through}
abbr[title]{text-decoration:underline dotted}
mark{background-color:#ff0;color:#000}
a[href]{color:LinkText;text-decoration:underline;cursor:pointer}
center,caption{text-align:-webkit-center}
hr{margin:.5em auto;border:1px inset;color:gray;overflow:hidden}
table{display:table;box-sizing:border-box;border-spacing:2px;text-indent:0}
thead{display:table-header-group}
tbody{display:table-row-group}
tfoot{display:table-footer-group}
tr{display:table-row}
td,th{display:table-cell}
caption{display:table-caption}
td,th{padding:1px}
thead,tbody,tfoot,table>tr{vertical-align:middle}
tr{vertical-align:inherit}
thead,tbody,tfoot,tr{border-color:inherit}
td,th{vertical-align:inherit}
fieldset{margin:0 2px;padding:.35em .75em .625em;border:2px groove light-dark(#efefef,#6b6b6b)}
legend{padding:0 2px}
summary{display:list-item;list-style-type:disclosure-closed}
details[open]>summary{list-style-type:disclosure-open}
details:not([open])>:not(summary){display:none}
button,input,select,textarea{margin:0;font-family:Arial;font-size:13.333333px;font-weight:normal;font-style:normal;line-height:normal;letter-spacing:normal;word-spacing:normal;text-transform:none;text-indent:0;text-shadow:none;text-align:start;color:light-dark(#000,#fff)}
button,select,meter,progress{user-select:none}
label,button,select,input[type=button i],input[type=submit i],input[type=reset i],input[type=checkbox i],input[type=radio i],input[type=range i],input[type=color i],input[type=file i]{cursor:default}
svg:not(:root){overflow:hidden}
input{padding:1px 2px;border:2px inset light-dark(#767676,#858585);background-color:light-dark(#fff,#3b3b3b)}
textarea{font-family:monospace;padding:2px;overflow:auto;border:1px solid light-dark(#767676,#858585);background-color:light-dark(#fff,#3b3b3b)}
select{border:1px solid light-dark(#767676,#858585);background-color:light-dark(#fff,#3b3b3b);box-sizing:border-box;white-space:pre}
button,input[type=button i],input[type=submit i],input[type=reset i]{padding:1px 6px;border:2px outset light-dark(#000,#fff);background-color:light-dark(#efefef,#6b6b6b);box-sizing:border-box;text-align:center}
input[type=checkbox i],input[type=radio i]{margin:3px 3px 3px 4px;padding:0;border:0;background-color:transparent;box-sizing:border-box}
input[type=radio i]{margin:3px 3px 0 5px}
input[type=range i]{margin:2px;padding:0;border:0;background-color:transparent;color:light-dark(#101010,#fff)}
input[type=color i]{width:50px;height:27px;border:1px solid light-dark(#000,#fff);background-color:light-dark(#efefef,#6b6b6b);box-sizing:border-box}
input[type=file i]{padding:0;border:0;background-color:transparent}
input[type=file i]::file-selector-button{margin:0 4px 0 0;padding:2px 7px;border:1px solid light-dark(#767676,#858585);border-radius:2px;background-color:light-dark(#efefef,#6b6b6b)}
input:is([type=date i],[type=time i],[type=datetime-local i],[type=month i],[type=week i]){padding:0;padding-inline-start:1px;font-family:monospace}
input[type=search i]{box-sizing:border-box}
summary{list-style-position:inside}
button:disabled,input[type=button i]:disabled,input[type=submit i]:disabled,input[type=reset i]:disabled{color:light-dark(rgba(16,16,16,.3),rgba(255,255,255,.3));background-color:light-dark(rgba(239,239,239,.3),rgba(19,1,1,.3));border-color:light-dark(rgba(118,118,118,.3),rgba(195,195,195,.3))}
input:disabled,textarea:disabled{color:light-dark(#545454,#aaa)}
input:disabled:not([type=checkbox i],[type=radio i],[type=range i],[type=color i],[type=file i],[type=button i],[type=submit i],[type=reset i]),textarea:disabled{background-color:light-dark(rgba(239,239,239,.3),rgba(59,59,59,.3));border-color:rgba(118,118,118,.3)}
select:disabled{opacity:.7;color:light-dark(#808080,#aaa);border-color:rgba(118,118,118,.3)}
:focus-visible{outline:auto 1px -webkit-focus-ring-color}
input:focus-visible,textarea:focus-visible,select:focus-visible{outline-offset:0}
:-webkit-any-link:focus-visible{outline-offset:1px}
progress,meter{width:10em;height:1em;vertical-align:-.2em;box-sizing:border-box}
meter{width:5em}
iframe{border:2px inset}
q::before{content:open-quote}
q::after{content:close-quote}
noembed,noframes{display:none}
select:not([multiple]):not([size]) :is(option:not(:checked),hr){display:none}
select[multiple] option,select[size]:not([size='0']):not([size='1']) option{padding:0 2px 1px;min-height:1.2em;line-height:normal!important}
select[multiple] option:checked,select[size]:not([size='0']):not([size='1']) option:checked{background-color:light-dark(#cecece,#545454)}
select[multiple],select[size]:not([size='0']):not([size='1']){overflow-y:auto;vertical-align:text-bottom}
dialog{position:absolute;left:0;right:0;width:fit-content;height:fit-content;margin:auto;border:solid;padding:1em;background-color:#fff;color:#000}
dialog:-internal-dialog-in-top-layer{position:fixed;top:0;bottom:0;max-width:calc(100% - 6px - 2em);max-height:calc(100% - 6px - 2em);overflow:auto;user-select:text;visibility:visible}
dialog::backdrop{background:rgba(0,0,0,.1)}
[popover]{position:fixed;inset:0;width:fit-content;height:fit-content;margin:auto;border:solid;padding:.25em;overflow:auto;color:CanvasText;background-color:Canvas}
[popover]:not(:popover-open):not(dialog[open]){display:none}
dialog:popover-open{display:block}
[popover]:popover-open::backdrop{pointer-events:none!important;background-color:transparent}
";

pub(crate) const FULLSCREEN_UA: &str = "
:not(:root):fullscreen{position:fixed!important;inset:0!important;margin:0!important;box-sizing:border-box!important;min-width:0!important;max-width:none!important;min-height:0!important;max-height:none!important;width:100%!important;height:100%!important;transform:none!important;object-fit:contain}
iframe:fullscreen{border:none!important;padding:0!important}
:not(:root):fullscreen::backdrop{background:black}
";

thread_local! {
    pub(crate) static UA_SHEET: Rc<Stylesheet> = Rc::new(Stylesheet::parse(UA));
    pub(crate) static FULLSCREEN_SHEET: Rc<Stylesheet> = Rc::new(Stylesheet::parse(&format!("{UA}{FULLSCREEN_UA}")));
    #[cfg(target_os = "macos")]
    pub(crate) static GLASS_SHEET: Rc<Stylesheet> = Rc::new(Stylesheet::parse(include_str!("tailwind/liquid-glass.css")));
}

pub(crate) const SVG: [Prop; 43] = [
    Prop::CLIP_PATH,
    Prop::CLIP_RULE,
    Prop::COLOR,
    Prop::DISPLAY,
    Prop::FILL,
    Prop::FILL_OPACITY,
    Prop::FILL_RULE,
    Prop::FILTER,
    Prop::FLOOD_COLOR,
    Prop::FLOOD_OPACITY,
    Prop::FONT_FAMILY,
    Prop::FONT_SIZE,
    Prop::FONT_STYLE,
    Prop::FONT_WEIGHT,
    Prop::IMAGE_RENDERING,
    Prop::ISOLATION,
    Prop::LETTER_SPACING,
    Prop::LIGHTING_COLOR,
    Prop::MASK,
    Prop::MASK_TYPE,
    Prop::MIX_BLEND_MODE,
    Prop::OPACITY,
    Prop::PAINT_ORDER,
    Prop::SHAPE_RENDERING,
    Prop::STOP_COLOR,
    Prop::STOP_OPACITY,
    Prop::STROKE,
    Prop::STROKE_DASHARRAY,
    Prop::STROKE_DASHOFFSET,
    Prop::STROKE_LINECAP,
    Prop::STROKE_LINEJOIN,
    Prop::STROKE_MITERLIMIT,
    Prop::STROKE_OPACITY,
    Prop::STROKE_WIDTH,
    Prop::TEXT_ANCHOR,
    Prop::TEXT_DECORATION,
    Prop::TEXT_RENDERING,
    Prop::TRANSFORM,
    Prop::TRANSFORM_ORIGIN,
    Prop::VECTOR_EFFECT,
    Prop::VISIBILITY,
    Prop::WORD_SPACING,
    Prop::FONT_STRETCH,
];

pub(crate) fn presentation(name: &str) -> Option<Prop> {
    Prop::find(name).filter(|prop| {
        SVG.contains(prop) && !matches!(*prop, Prop::TRANSFORM | Prop::TRANSFORM_ORIGIN)
    })
}

pub(crate) fn svg_transform(value: &str) -> Option<String> {
    let mut result = Vec::new();
    for part in value.split_inclusive(')') {
        let part = part.trim_matches(|c: char| c == ',' || c.is_ascii_whitespace());
        if part.is_empty() {
            continue;
        }
        let (name, args) = part.strip_suffix(')')?.split_once('(')?;
        let args = args
            .split(|c: char| c == ',' || c.is_ascii_whitespace())
            .filter(|arg| !arg.is_empty())
            .map(|arg| arg.parse::<f32>().ok())
            .collect::<Option<Vec<_>>>()?;
        result.push(match (name.trim(), args.as_slice()) {
            ("matrix", [a, b, c, d, e, f]) => format!("matrix({a}, {b}, {c}, {d}, {e}, {f})"),
            ("translate", [x]) => format!("translate({x}px)"),
            ("translate", [x, y]) => format!("translate({x}px, {y}px)"),
            ("scale", [x]) => format!("scale({x})"),
            ("scale", [x, y]) => format!("scale({x}, {y})"),
            ("rotate", [a]) => format!("rotate({a}deg)"),
            ("rotate", [a, x, y]) => {
                format!(
                    "translate({x}px, {y}px) rotate({a}deg) translate({}px, {}px)",
                    -x, -y
                )
            }
            ("skewX", [a]) => format!("skewX({a}deg)"),
            ("skewY", [a]) => format!("skewY({a}deg)"),
            _ => return None,
        });
    }
    Some(if result.is_empty() {
        "none".into()
    } else {
        result.join(" ")
    })
}
