use super::*;

extern "C" fn waker() -> *mut c_void {
    std::ptr::null_mut()
}

extern "C" fn wake(_: *mut c_void) {}

extern "C" fn alloc(output: *mut c_void, length: usize) -> *mut u8 {
    let output = unsafe { &mut *output.cast::<Vec<u8>>() };
    *output = vec![0; length];
    output.as_mut_ptr()
}

// A decoder that makes one pixel from any data.
extern "C" fn decode_png(
    _: *const u8,
    _: usize,
    size: *mut u32,
    output: *mut c_void,
    alloc: Alloc,
) -> bool {
    let pixel = alloc(output, 4);
    unsafe {
        *size = 1;
        *size.add(1) = 1;
        pixel.copy_from(b"\x01\x02\x03\x04".as_ptr(), 4);
    }
    true
}

struct Test(*mut Terminal);

impl Drop for Test {
    fn drop(&mut self) {
        unsafe { apps_terminal_free(self.0) };
    }
}

impl Test {
    fn new(cols: u16, rows: u16) -> Self {
        let host = Host {
            waker,
            wake,
            drop_waker: wake,
            decode_png,
        };
        let terminal = unsafe { apps_terminal_new(&host, cols, rows) };
        assert!(!terminal.is_null());
        Self(terminal)
    }

    fn feed(&self, text: &str) {
        unsafe { apps_terminal_write(self.0, OUTPUT, text.as_ptr(), text.len()) };
    }

    fn poll(&self) -> String {
        let mut out = Vec::new();
        unsafe { apps_terminal_poll(self.0, (&raw mut out).cast(), alloc) };
        String::from_utf8(out).unwrap()
    }

    fn info(&self) -> String {
        let mut out = Vec::new();
        unsafe { apps_terminal_info(self.0, (&raw mut out).cast(), alloc) };
        String::from_utf8(out).unwrap()
    }

    fn text(&self, selected: bool) -> Option<String> {
        let mut out = Vec::new();
        unsafe { apps_terminal_text(self.0, selected, (&raw mut out).cast(), alloc) }
            .then(|| String::from_utf8(out).unwrap())
    }

    // The changed rows as (row, cells).
    fn frame(&self) -> (Frame, Vec<(u16, Vec<Cell>)>) {
        let mut frame: Frame = unsafe { std::mem::zeroed() };
        assert!(unsafe { apps_terminal_frame(self.0, &mut frame) });
        let cols = frame.cols as usize;
        let rows = (0..frame.count as usize)
            .map(|index| unsafe {
                (
                    *frame.ys.add(index),
                    std::slice::from_raw_parts(frame.cells.add(index * cols), cols).to_vec(),
                )
            })
            .collect();
        (frame, rows)
    }

    fn key(&self, name: &str, mods: u32, text: &str) -> bool {
        unsafe {
            apps_terminal_key(
                self.0,
                1,
                name.as_ptr(),
                name.len(),
                mods,
                text.as_ptr(),
                text.len(),
            )
        }
    }
}

#[test]
fn frames_have_only_the_rows_that_changed() {
    let test = Test::new(10, 4);
    test.feed("hi \x1b[1;31mred\x1b[0m\r\n\x1b[48;2;1;2;3m \x1b[0m\u{4e2d}e\u{301}");
    let (frame, rows) = test.frame();
    assert_eq!((frame.cols, frame.rows, frame.full), (10, 4, 1));
    assert_eq!(rows.len(), 4);
    let first = &rows[0].1;
    assert_eq!(first[0].ch, 'h' as u32);
    assert_eq!((first[0].fg, first[0].flags), (0, 0));
    assert_eq!(first[3].ch, 'r' as u32);
    assert_eq!(first[3].flags & 1, 1);
    assert_eq!(first[3].fg >> 24, 1);
    let second = &rows[1].1;
    assert_eq!(second[0].bg, 0x01010203);
    assert_eq!(second[1].ch, 0x4e2d);
    assert_eq!(second[1].flags & 1 << 11, 1 << 11);
    assert_eq!(second[2].flags & 1 << 12, 1 << 12);
    assert_eq!((second[3].ch, second[3].extra), ('e' as u32, 1));
    assert_eq!(unsafe { *frame.extras }, 0x301);
    assert_eq!((frame.cursor, frame.cursor_x, frame.cursor_y), (2, 4, 1));

    assert_eq!(test.frame().1.len(), 0);
    test.feed("\x1b[4;1Hx");
    let (frame, rows) = test.frame();
    assert_eq!(frame.full, 0);
    assert_eq!(rows.iter().map(|row| row.0).collect::<Vec<_>>(), [1, 3]);
    assert_eq!(test.text(false).unwrap(), "hi red\n \u{4e2d}e\u{301}\n\nx");
    assert_eq!(test.text(true), None);
}

#[test]
fn input_without_a_program_goes_to_the_page() {
    let test = Test::new(20, 4);
    assert!(test.key("a", 0, "a"));
    assert!(test.key("c", 2, ""));
    assert!(test.key("up", 0, ""));
    assert!(test.key("enter", 0, "\n"));
    assert!(test.key("f5", 0, ""));
    // A modifier alone sends nothing.
    assert!(!test.key("shift", 1, ""));
    assert_eq!(
        test.poll(),
        r#"[["data","a"],["data","\u0003"],["data","\u001b[A"],["data","\u000d"],["data","\u001b[15~"]]"#
    );
    // Option types a character on macOS, or is an Alt key.
    assert!(test.key("f", 4, "\u{192}"));
    assert!(test.key("f", 4 | 16, "\u{192}"));
    let typed = if cfg!(target_os = "macos") {
        "\u{192}"
    } else {
        "\\u001b\u{192}"
    };
    assert_eq!(
        test.poll(),
        format!(r#"[["data","{typed}"],["data","\u001bf"]]"#)
    );
    // Application cursor keys and a bracketed paste.
    test.feed("\x1b[?1h\x1b[?2004h");
    assert!(test.key("up", 0, ""));
    unsafe { apps_terminal_write(test.0, PASTE, "a\nb".as_ptr(), 3) };
    assert_eq!(
        test.poll(),
        r#"[["data","\u001bOA"],["data","\u001b[200~a\u000ab\u001b[201~"]]"#
    );
    // The terminal answers queries of the program.
    test.feed("\x1b[6n\x1b]7501;?\x1b\\");
    let replies = test.poll();
    assert!(replies.contains(r#"\u001b[1;1R"#), "{replies}");
    assert!(replies.contains(r#"\u001b]7501;?"#), "{replies}");
}

#[test]
fn events_report_the_title_the_directory_and_the_bell() {
    let test = Test::new(20, 4);
    test.feed("\x1b]2;my \"title\"\x07\x1b]7;file://host/tmp/a%20b\x07\x07");
    assert_eq!(
        test.poll(),
        r#"[["title","my \"title\""],["cwd","/tmp/a b"],["bell"]]"#
    );
    let info = test.info();
    assert!(info.contains(r#""title":"my \"title\"""#), "{info}");
    assert!(info.contains(r#""cwd":"/tmp/a b""#), "{info}");
    assert!(info.contains(r#""process":null"#), "{info}");
    test.feed("\x1bc");
    assert_eq!(test.poll(), r#"[["title",""],["cwd",""]]"#);
    assert_eq!(test.poll(), "[]");
}

#[test]
fn program_status_records_follow_the_protocol() {
    let test = Test::new(20, 4);
    let status = || {
        let events = test.poll();
        events
            .strip_prefix(r#"[["status","#)
            .and_then(|rest| rest.strip_suffix("]]"))
            .unwrap_or(&events)
            .to_owned()
    };
    // "Deploying" and "Pushing image", as in the examples of the specification.
    test.feed("\x1b]7501;state=working:app=deploy:msg=RGVwbG95aW5n\x1b\\");
    test.feed("\x1b]7501;state=working:id=us-east:title=VVMgRWFzdA==:progress=40:msg=UHVzaGluZyBpbWFnZQ==\x07");
    assert_eq!(
        status(),
        concat!(
            r#"[{"id":"","state":"working","kind":null,"progress":null,"app":"deploy","title":"","message":"Deploying"},"#,
            r#"{"id":"us-east","state":"working","kind":null,"progress":40,"app":"deploy","title":"US East","message":"Pushing image"}]"#
        )
    );
    // A report replaces its record, and invalid reports change nothing.
    test.feed("\x1b]7501;state=blocked:kind=auth:id=us-east\x07");
    test.feed("\x1b]7501;state=unknown:id=us-east\x07\x1b]7501;state=done:msg=A\x07");
    assert_eq!(
        status(),
        concat!(
            r#"[{"id":"","state":"working","kind":null,"progress":null,"app":"deploy","title":"","message":"Deploying"},"#,
            r#"{"id":"us-east","state":"blocked","kind":"auth","progress":null,"app":"deploy","title":"","message":""}]"#
        )
    );
    assert_eq!(test.poll(), "[]");
    // A clear removes the record and the records below it.
    test.feed("\x1b]7501;state=done:id=us-east/a\x07\x1b]7501;state=done:id=us-eastern\x07");
    test.feed("\x1b]7501;state=clear:id=us-east\x07");
    assert_eq!(
        status(),
        concat!(
            r#"[{"id":"","state":"working","kind":null,"progress":null,"app":"deploy","title":"","message":"Deploying"},"#,
            r#"{"id":"us-eastern","state":"done","kind":null,"progress":null,"app":"deploy","title":"","message":""}]"#
        )
    );
    // A new shell prompt ends the work, but what finished stays until the user types.
    test.feed("\x1b]133;A\x07");
    assert_eq!(
        status(),
        r#"[{"id":"us-eastern","state":"done","kind":null,"progress":null,"app":"","title":"","message":""}]"#
    );
    assert!(test.key("a", 0, "a"));
    assert_eq!(test.poll(), r#"[["data","a"],["status",[]]]"#);
    // A full reset removes all records.
    test.feed("\x1b]7501;state=error\x07\x1bc");
    assert_eq!(test.poll(), r#"[["title",""],["cwd",""],["status",[]]]"#);
    test.feed("\x1bc");
    assert_eq!(test.poll(), r#"[["title",""],["cwd",""]]"#);
    // The oldest record leaves when there are too many.
    for index in 0..RECORDS + 1 {
        test.feed(&format!("\x1b]7501;state=idle:id=r{index}\x07"));
    }
    let records = status();
    assert_eq!(records.matches("\"id\"").count(), RECORDS);
    assert!(!records.contains(r#""id":"r0""#) && records.contains(r#""id":"r256""#));
}

#[test]
fn the_mouse_selects_text_or_goes_to_the_program() {
    let test = Test::new(20, 4);
    test.feed("hello world\r\nsecond line");
    unsafe { apps_terminal_resize(test.0, 20, 4, 10, 20) };
    let mouse = |kind, button, x: f64, y: f64, held| unsafe {
        apps_terminal_mouse(test.0, kind, button, 0, x, y, held)
    };
    assert!(!mouse(0, 1, 2., 8., true));
    assert!(!mouse(2, 0, 46., 8., true));
    assert!(!mouse(1, 1, 46., 8., false));
    assert_eq!(test.text(true).unwrap(), "hello");
    let (_, rows) = test.frame();
    let selected = |cells: &[Cell]| {
        cells
            .iter()
            .filter(|cell| cell.flags & 1 << 13 != 0)
            .count()
    };
    assert_eq!(selected(&rows[0].1), 5);
    unsafe { apps_terminal_control(test.0, SELECT_ALL, 0.) };
    assert_eq!(test.text(true).unwrap(), "hello world\nsecond line");
    unsafe { apps_terminal_control(test.0, SELECT_NONE, 0.) };
    assert_eq!(test.text(true), None);
    assert_eq!(test.poll(), "[]");
    // With mouse tracking, the program gets the buttons and the wheel.
    test.feed("\x1b[?1000h\x1b[?1006h");
    assert!(mouse(0, 1, 25., 30., true));
    assert!(mouse(1, 1, 25., 30., false));
    unsafe { apps_terminal_wheel(test.0, -1, 0, 25., 30.) };
    assert_eq!(
        test.poll(),
        r#"[["data","\u001b[<0;3;2M"],["data","\u001b[<0;3;2m"],["data","\u001b[<64;3;2M"]]"#
    );
}

#[test]
fn the_wheel_scrolls_the_scrollback() {
    let test = Test::new(10, 2);
    test.feed("1\r\n2\r\n3\r\n4");
    let top = |test: &Test| char::from_u32(test.frame().1[0].1[0].ch).unwrap();
    assert_eq!(top(&test), '3');
    unsafe { apps_terminal_wheel(test.0, -2, 0, 0., 0.) };
    assert_eq!(top(&test), '1');
    assert!(
        test.info()
            .contains(r#""bottom":false,"total":4,"offset":0"#),
        "{}",
        test.info()
    );
    // Typing goes back to the newest rows.
    assert!(test.key("a", 0, "a"));
    assert_eq!(top(&test), '3');
    unsafe { apps_terminal_control(test.0, RESET, 0.) };
    assert_eq!(test.text(false).unwrap(), "");
}

#[test]
fn frames_have_the_images_of_the_screen() {
    let test = Test::new(10, 4);
    unsafe { apps_terminal_resize(test.0, 10, 4, 10, 20) };
    let placements = |test: &Test| {
        let frame = test.frame().0;
        (0..frame.placed as usize)
            .map(|index| unsafe { *frame.placements.add(index) })
            .collect::<Vec<_>>()
    };
    let pixels = |placement: &Placement, generation: u64| {
        let (mut size, mut out) = ([0u32; 2], Vec::<u8>::new());
        unsafe {
            apps_terminal_image(
                test.0,
                placement.id,
                generation,
                size.as_mut_ptr(),
                (&raw mut out).cast(),
                alloc,
            )
        }
        .then_some((size, out))
    };
    assert_eq!(placements(&test), []);
    // Two RGBA pixels in two columns and one row, below the text, and a PNG image above it.
    test.feed("\x1b_Ga=T,f=32,s=2,v=1,i=7,c=2,r=1,z=-1;/wAA/wD/AIA=\x1b\\");
    test.feed("\x1b_Ga=T,f=100,i=8;iVBORw==\x1b\\");
    let placed = placements(&test);
    assert_eq!(placed.len(), 2, "{placed:?}");
    let [below, above] = [placed[0], placed[1]];
    assert_eq!((below.id, below.layer, below.col, below.row), (7, 1, 0, 0));
    assert_eq!(
        (below.width, below.height, below.source),
        (20, 20, [0, 0, 2, 1])
    );
    assert_eq!(
        pixels(&below, below.image),
        Some(([2, 1], vec![0, 0, 255, 255, 0, 255, 0, 128]))
    );
    assert_eq!((above.id, above.layer, above.col), (8, 2, 2));
    assert_eq!(
        pixels(&above, above.image),
        Some(([1, 1], vec![3, 2, 1, 4]))
    );
    assert_eq!(pixels(&above, below.image), None);
    // RGB pixels get an alpha channel, and a new picture of an image blends over the old one.
    test.feed("\x1b_Ga=T,f=24,s=1,v=1,i=11,C=1;/wAA\x1b\\");
    let found = |test: &Test| {
        *placements(test)
            .iter()
            .find(|image| image.id == 11)
            .unwrap()
    };
    let rgb = found(&test);
    assert_eq!(
        pixels(&rgb, rgb.image),
        Some(([1, 1], vec![0, 0, 255, 255]))
    );
    test.feed("\x1b_Ga=f,i=11,r=1,f=32,s=1,v=1;AP8AgA==\x1b\\");
    let blended = found(&test);
    assert_ne!(blended.image, rgb.image);
    let (_, pixel) = pixels(&blended, blended.image).unwrap();
    assert!(
        pixel[0] == 0 && pixel[1] > 100 && pixel[2] > 100 && pixel[3] == 255,
        "{pixel:?}"
    );
    test.feed("\x1b_Ga=d,d=I,i=11\x1b\\");
    // New pixels for an image give it a new generation.
    test.feed("\x1b_Ga=t,f=32,s=1,v=1,i=7;AAAAAA==\x1b\\");
    assert_eq!(pixels(&below, below.image), None);
    // Images move with the text.
    test.feed("\x1b_Ga=d\x1b\\\x1b[2J\x1b[3;1H\x1b_Ga=T,f=100,i=9;iVBORw==\x1b\\\n\n\n");
    let placed = placements(&test);
    assert_eq!((placed.len(), placed[0].id, placed[0].row), (1, 9, 0));
    // The cells of a placeholder draw the image, not the character.
    test.feed("\x1b[2J\x1b[H\x1b_Ga=T,U=1,f=100,i=5,c=1,r=1;iVBORw==\x1b\\\x1b[38;5;5m\u{10EEEE}\u{305}\u{305}");
    let (frame, rows) = test.frame();
    assert_eq!(rows[0].1[0].ch, 0);
    assert_eq!(frame.placed, 1);
}

#[cfg(unix)]
#[test]
fn a_program_runs_in_a_pty() {
    let test = Test::new(40, 4);
    let mut error = Vec::new();
    let argv = "/bin/sh\0-c\0printf 'in %s' \"$PWD\"; read line; echo \"got $line\"; exit 7\0";
    let pid = unsafe {
        apps_terminal_spawn(
            test.0,
            argv.as_ptr(),
            argv.len(),
            std::ptr::null(),
            0,
            "/".as_ptr(),
            1,
            (&raw mut error).cast(),
            alloc,
        )
    };
    assert!(pid > 0, "{}", String::from_utf8_lossy(&error));
    let wait = |what: &mut dyn FnMut() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !what() {
            assert!(Instant::now() < deadline, "{:?}", test.text(false));
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    wait(&mut || test.text(false).is_some_and(|text| text.contains("in /")));
    let info = test.info();
    assert!(
        info.contains(&format!(r#""process":{{"pid":{pid},"name":""#)),
        "{info}"
    );
    assert!(!info.contains(r#""name":"""#), "{info}");
    assert!(info.contains(r#""cwd":"/""#), "{info}");
    unsafe { apps_terminal_write(test.0, INPUT, "hi\r".as_ptr(), 3) };
    let mut events = String::new();
    wait(&mut || {
        events.push_str(&test.poll());
        events.contains("exit")
    });
    assert!(events.contains(r#"["exit",7,null]"#), "{events}");
    wait(&mut || test.text(false).is_some_and(|text| text.contains("got hi")));
    assert!(test.info().contains(r#""pid":null"#));
}
