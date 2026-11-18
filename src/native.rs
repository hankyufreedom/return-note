#![allow(unsafe_op_in_unsafe_fn)]
use crate::{
    appearance::{ACCENT, INK, INPUT, LINE, MUTED, PAPER, Painter, SELECT, WHITE, category_colors},
    network,
};
use return_note::{
    ai::Classification,
    model::{Board, MAX_TAGS, Status, now},
    storage::Storage,
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    path::PathBuf,
    ptr::{null, null_mut},
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    System::{LibraryLoader::GetModuleHandleW, Threading::CreateMutexW},
    UI::{
        Controls::*,
        HiDpi::*,
        Input::KeyboardAndMouse::*,
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};

const ZOOM_LEVELS: [u16; 5] = [70, 85, 100, 110, 125];
const CLASS: &str = "ReturnNote.Window.v1";
const RESULT: u32 = WM_APP + 1;
const DRAG_START: u32 = WM_APP + 2;
const DRAG_MOVE: u32 = WM_APP + 3;
const DRAG_DROP: u32 = WM_APP + 4;
const DRAG_CANCEL: u32 = WM_APP + 5;
unsafe fn visual_scale(hwnd: HWND) -> f64 {
    VIEW.with(|view| view.borrow().as_ref().map(|v| v.scale))
        .unwrap_or_else(|| GetDpiForWindow(hwnd).max(96) as f64 / 96.0)
}
fn mouse_point(value: LPARAM) -> POINT {
    POINT {
        x: value as u16 as i16 as i32,
        y: (value >> 16) as u16 as i16 as i32,
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe fn text(h: HWND) -> String {
    let n = GetWindowTextLengthW(h);
    let mut b = vec![0u16; (n + 1) as usize];
    let got = GetWindowTextW(h, b.as_mut_ptr(), n + 1);
    String::from_utf16_lossy(&b[..got as usize])
}
unsafe fn set(h: HWND, s: &str) {
    if text(h) != s {
        SetWindowTextW(h, wide(s).as_ptr());
    }
}
unsafe fn message(h: HWND, s: &str) {
    MessageBoxW(
        h,
        wide(s).as_ptr(),
        wide("回來").as_ptr(),
        MB_OK | MB_ICONINFORMATION,
    );
}

// Win32 owner-draw messages can arrive synchronously during a model update.
// Drawing from a separate immutable snapshot keeps those callbacks reentrant.
struct DisplayRow {
    id: u64,
    title: String,
    subtitle: String,
    timing: bool,
    category: usize,
    status: Status,
    reminder_due: bool,
}
struct Visual {
    hwnd: HWND,
    list: HWND,
    font: HFONT,
    bold: HFONT,
    tiny: HFONT,
    scale: f64,
    managing: bool,
    settings: bool,
    editing: bool,
    searching: bool,
    filter: usize,
    topmost: bool,
    sidebar_collapsed: bool,
    drag_source: Option<u64>,
    drag_indicator: Option<i32>,
    category_filter: Option<usize>,
    labels: Vec<String>,
    category_counts: Vec<usize>,
    selected_category: usize,
    rows: Vec<DisplayRow>,
    toast: String,
    toast_error: bool,
}
thread_local! {
    static VIEW: RefCell<Option<Rc<Visual>>> = const { RefCell::new(None) };
    // Readable only in an isolated UI test process. No user text or credentials.
    static PAINT_STATS: Cell<[usize; 6]> = const { Cell::new([0; 6]) };
}
fn count_event(index: usize) {
    PAINT_STATS.with(|stats| {
        let mut values = stats.get();
        values[index] += 1;
        stats.set(values);
    });
}
impl Visual {
    unsafe fn paint(&self, dc: HDC) {
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(self.hwnd, &mut r);
        let w = (r.right as f64 / self.scale) as i32;
        let h = (r.bottom as f64 / self.scale) as i32;
        let p = Painter {
            dc,
            scale: self.scale,
        };
        p.fill(0, 0, w, h, PAPER);
        let left = if self.sidebar_collapsed { 14 } else { 110 };
        let title = if self.managing {
            "管理標籤"
        } else if self.settings {
            "設定"
        } else if self.editing {
            "便條"
        } else if self.filter == 4 {
            "已完成"
        } else if self.sidebar_collapsed
            && let Some(i) = self.category_filter
        {
            &self.labels[i]
        } else {
            "回來"
        };
        p.text(
            title,
            self.bold,
            INK,
            56,
            10,
            128,
            26,
            DT_SINGLELINE | DT_VCENTER,
        );
        if !self.settings && !self.editing && !self.rows.is_empty() {
            p.text(
                &format!("{}", self.rows.len()),
                self.tiny,
                MUTED,
                {
                    let old = SelectObject(dc, self.bold);
                    let mut size: SIZE = std::mem::zeroed();
                    let title_text = wide(title);
                    GetTextExtentPoint32W(
                        dc,
                        title_text.as_ptr(),
                        title_text.len() as i32 - 1,
                        &mut size,
                    );
                    SelectObject(dc, old);
                    56 + (size.cx as f64 / self.scale) as i32 + 8
                },
                12,
                32,
                22,
                DT_SINGLELINE | DT_VCENTER,
            );
        }
        p.stroke(&[(22, 48), (w - 22, 48)], LINE, 1);
        if self.managing {
            p.round(24, 186, w - 48, 38, 7, INPUT);
        } else if self.settings {
            p.round(24, 104, w - 48, 40, 8, INPUT);
            p.round(24, 182, w - 48, 40, 8, INPUT);
        } else if self.editing {
            p.round(24, 248, w - 48, 120, 10, INPUT);
        } else {
            if !self.sidebar_collapsed {
                p.stroke(&[(102, 60), (102, h - 80)], LINE, 1);
            }
            if self.searching {
                p.round(left, 56, w - left - 20, 38, 7, INPUT);
            }
            if self.rows.is_empty() {
                p.text(
                    if self.filter == 4 {
                        "還沒有完成的便條"
                    } else if self.searching {
                        "沒有找到便條"
                    } else if self.category_filter.is_some() {
                        "這個分類還沒有便條"
                    } else {
                        "想到什麼，就記在這裡。"
                    },
                    self.font,
                    MUTED,
                    left + 4,
                    if self.searching { 134 } else { 94 },
                    w - left - 28,
                    60,
                    DT_WORDBREAK | DT_CENTER,
                );
            }
            p.round(14, h - 60, w - 28, 42, 9, INPUT);
        }
        if !self.toast.is_empty() && !self.settings {
            let color = if self.toast_error {
                crate::appearance::rgb(160, 70, 45)
            } else {
                MUTED
            };
            p.text(
                &self.toast,
                self.tiny,
                color,
                if self.toast_error && !self.editing {
                    left + 2
                } else {
                    26
                },
                if self.toast_error && !self.settings && !self.editing {
                    h - 112
                } else {
                    h - 17
                },
                w - if self.toast_error && !self.editing {
                    left + 26
                } else {
                    52
                },
                if self.toast_error { 44 } else { 16 },
                DT_WORDBREAK | DT_END_ELLIPSIS,
            );
        }
    }
    unsafe fn draw(&self, d: &DRAWITEMSTRUCT) {
        let p = Painter {
            dc: d.hDC,
            scale: self.scale,
        };
        let x = (d.rcItem.left as f64 / self.scale) as i32;
        let y = (d.rcItem.top as f64 / self.scale) as i32;
        let w = ((d.rcItem.right - d.rcItem.left) as f64 / self.scale) as i32;
        let h = ((d.rcItem.bottom - d.rcItem.top) as f64 / self.scale) as i32;
        p.fill(x, y, w, h, if d.CtlID == 111 { INPUT } else { PAPER });
        if d.CtlType == ODT_BUTTON {
            let id = d.CtlID as i32;
            let hover = !GetPropW(d.hwndItem, wide("rn.hover").as_ptr()).is_null();
            let focus = d.itemState & ODS_FOCUS != 0;
            let pressed = d.itemState & ODS_SELECTED != 0;
            if (300..=332).contains(&id) {
                if (id - 300) as usize >= self.category_counts.len() {
                    return;
                }
                let category = (id > 300).then_some((id - 301) as usize);
                let active = self.category_filter == category;
                let (ink, tint) = category.map(category_colors).unwrap_or((INK, INPUT));
                if active || hover || focus {
                    p.round(x, y, w, h, 6, if active { tint } else { INPUT });
                }
                if let Some(index) = category {
                    p.round(x + 8, y + (h - 5) / 2, 5, 5, 2, category_colors(index).0);
                }
                p.text(
                    if let Some(index) = category {
                        &self.labels[index]
                    } else {
                        "全部"
                    },
                    self.tiny,
                    if active { ink } else { MUTED },
                    x + 20,
                    y,
                    w - 36,
                    h,
                    DT_SINGLELINE | DT_VCENTER,
                );
                p.text(
                    &self.category_counts[(id - 300) as usize].to_string(),
                    self.tiny,
                    MUTED,
                    x + w - 16,
                    y,
                    12,
                    h,
                    DT_SINGLELINE | DT_VCENTER | DT_RIGHT,
                );
                return;
            }
            if id == 142 {
                let (ink, tint) = category_colors(self.selected_category);
                p.round(x, y, w, h, 6, tint);
                p.text(
                    &format!("{}  ▾", self.labels[self.selected_category]),
                    self.tiny,
                    ink,
                    x + 8,
                    y,
                    w - 16,
                    h,
                    DT_SINGLELINE | DT_VCENTER,
                );
                return;
            }
            if [111, 206, 138].contains(&id) {
                p.round(x, y, w, h, if id == 111 { 7 } else { 8 }, ACCENT);
            } else if hover || focus || pressed {
                p.round(x, y, w, h, 7, SELECT);
            } else if [207, 209, 214, 215].contains(&id) {
                p.round(x, y, w, h, 7, INPUT);
            }
            let color = if [111, 206, 138].contains(&id) {
                WHITE
            } else {
                INK
            };
            if [111, 112, 113, 116, 117, 118, 119, 210].contains(&id) {
                p.icon(
                    id,
                    x + (w - 20) / 2,
                    y + (h - 20) / 2,
                    if id == 113 && self.topmost {
                        ACCENT
                    } else {
                        color
                    },
                    if id == 118 {
                        !self.sidebar_collapsed
                    } else {
                        self.topmost
                    },
                );
            } else {
                p.text(
                    &text(d.hwndItem),
                    self.tiny,
                    color,
                    x + 4,
                    y,
                    w - 8,
                    h,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                );
            }
            return;
        }
        let mut viewport: RECT = std::mem::zeroed();
        GetClientRect(d.hwndItem, &mut viewport);
        if d.rcItem.top > 0 && d.rcItem.bottom > viewport.bottom {
            return;
        }
        if d.itemID == u32::MAX {
            return;
        }
        let Some(n) = self.rows.get(d.itemID as usize) else {
            return;
        };
        if d.itemState & ODS_SELECTED != 0 && GetFocus() == self.list {
            p.round(x, y + 1, w, h - 3, 8, SELECT);
        }
        if self.drag_source == Some(n.id) {
            p.round(x, y + 1, w, h - 3, 6, INPUT);
        }
        if n.status != Status::Done {
            if n.timing {
                p.round(x + 1, y + 11, 13, 13, 6, ACCENT);
                for dx in [5, 9] {
                    p.fill(x + dx, y + 15, 1, 5, WHITE);
                }
            } else {
                p.round(x + 1, y + 11, 13, 13, 6, MUTED);
                p.round(x + 2, y + 12, 11, 11, 5, PAPER);
                p.stroke(
                    &[(x + 7, y + 14), (x + 7, y + 17), (x + 10, y + 18)],
                    MUTED,
                    1,
                );
            }
        }
        p.check(x + 20, y + 10, n.status == Status::Done);
        let (category_ink, category_tint) = category_colors(n.category);
        p.round(x + w - 74, y + 8, 70, 20, 5, category_tint);
        p.text(
            &self.labels[n.category],
            self.tiny,
            category_ink,
            x + w - 72,
            y + 8,
            66,
            20,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
        );
        let subtitle = &n.subtitle;
        let title_h = h - 12 - if subtitle.is_empty() { 0 } else { 20 };
        p.text(
            &n.title,
            self.font,
            if n.status == Status::Done { MUTED } else { INK },
            x + 44,
            y + 5,
            w - 126,
            title_h,
            DT_WORDBREAK | DT_END_ELLIPSIS,
        );
        if !subtitle.is_empty() {
            p.text(
                subtitle,
                self.tiny,
                if n.status == Status::Now || n.reminder_due {
                    ACCENT
                } else {
                    MUTED
                },
                x + 44,
                y + h - 25,
                w - 48,
                20,
                DT_SINGLELINE | DT_END_ELLIPSIS | DT_VCENTER,
            );
        }
    }
}

struct DragState {
    source: u64,
    rows: Vec<u64>,
    start: POINT,
    point: POINT,
    active: bool,
    slot: Option<usize>,
    indicator: Option<i32>,
    scroll_direction: i32,
}
struct Job {
    id: u64,
    revision: u64,
    original: String,
    fixed_category: Option<String>,
}
struct Outcome {
    id: u64,
    revision: u64,
    result: Result<Classification, String>,
}
struct App {
    hwnd: HWND,
    controls: HashMap<i32, HWND>,
    font: HFONT,
    bold: HFONT,
    big: HFONT,
    tiny: HFONT,
    editing: bool,
    searching: bool,
    main_size: (i32, i32),
    drag: Option<DragState>,
    toast: String,
    toast_error: bool,
    tips: Vec<Vec<u16>>,
    board: Board,
    storage: Storage,
    selected: Option<u64>,
    rows: Vec<u64>,
    managing: bool,
    settings: bool,
    dirty: bool,
    filter: usize,
    category_filter: Option<usize>,
    tx: Sender<Outcome>,
    rx: Receiver<Outcome>,
    queue: VecDeque<Job>,
    busy: bool,
    in_flight: Option<u64>,
    ai_paused: bool,
    ai_status: String,
    hotkey: bool,
    scale: f64,
}
impl App {
    fn h(&self, id: i32) -> HWND {
        self.controls.get(&id).copied().unwrap_or(null_mut())
    }
    unsafe fn control(&mut self, id: i32, class: &str, label: &str, style: u32) {
        let h = CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(label).as_ptr(),
            WS_CHILD
                | style
                | if class == "BUTTON" {
                    BS_OWNERDRAW as u32
                } else {
                    0
                },
            0,
            0,
            10,
            10,
            self.hwnd,
            id as usize as _,
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(h, WM_SETFONT, self.font as usize, 1);
        self.controls.insert(id, h);
        SetWindowSubclass(h, Some(control_proc), id as usize, id as usize);
    }
    unsafe fn init(&mut self) {
        self.control(
            110,
            "EDIT",
            "",
            WS_TABSTOP | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | ES_WANTRETURN as u32,
        );
        SendMessageW(self.h(110), EM_SETLIMITTEXT, 8000, 0);
        for (id, label) in [
            (111, "新增便條 · Enter（Shift+Enter 換行）"),
            (112, "更多選項"),
            (113, "置頂"),
            (116, "返回便條"),
            (117, "收起視窗"),
            (118, "收合分類"),
            (119, "依標籤排序"),
            (135, "現在做"),
            (136, "等結果"),
            (137, "稍後"),
            (138, "完成"),
            (139, "10 分後回看"),
            (140, "看原文"),
            (141, "重新分類"),
            (142, "變更分類"),
            (206, "儲存設定"),
            (215, "測試連線"),
            (207, "使用本機分類"),
            (208, "移除金鑰"),
            (209, "匯出便條"),
            (210, "返回便條"),
            (212, "結束程式"),
            (214, "開啟資料夾"),
        ] {
            self.control(id, "BUTTON", label, WS_TABSTOP);
        }
        self.control(300, "BUTTON", "全部分類", WS_TABSTOP);
        for index in 0..MAX_TAGS {
            self.control(301 + index as i32, "BUTTON", "", WS_TABSTOP);
        }
        self.control(333, "COMBOBOX", "", WS_TABSTOP | CBS_DROPDOWNLIST as u32);
        self.control(220, "COMBOBOX", "", WS_TABSTOP | CBS_DROPDOWNLIST as u32);
        self.control(221, "EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL as u32);
        for (id, label) in [(222, "新增"), (223, "改名"), (224, "刪除標籤")] {
            self.control(id, "BUTTON", label, WS_TABSTOP);
        }
        self.control(225, "STATIC", "標籤", 0);
        self.control(226, "STATIC", "名稱", 0);

        self.control(114, "COMBOBOX", "", CBS_DROPDOWNLIST as u32);
        for name in ["全部未完成", "現在做", "等結果", "稍後", "已完成"] {
            SendMessageW(self.h(114), CB_ADDSTRING, 0, wide(name).as_ptr() as isize);
        }
        SendMessageW(self.h(114), CB_SETCURSEL, 0, 0);
        self.control(115, "EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL as u32);
        self.control(
            120,
            "LISTBOX",
            "",
            WS_TABSTOP
                | LBS_NOTIFY as u32
                | LBS_OWNERDRAWVARIABLE as u32
                | LBS_HASSTRINGS as u32
                | LBS_NOINTEGRALHEIGHT as u32,
        );
        self.control(
            121,
            "STATIC",
            "捲動便條",
            windows_sys::Win32::System::SystemServices::SS_NOTIFY,
        );
        self.control(
            131,
            "EDIT",
            "",
            WS_TABSTOP | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32,
        );
        SendMessageW(self.h(131), EM_SETLIMITTEXT, 200, 0);
        SendMessageW(self.h(131), WM_SETFONT, self.big as usize, 1);
        self.control(132, "COMBOBOX", "", CBS_DROPDOWNLIST as u32);
        self.sync_tags();
        self.control(
            134,
            "EDIT",
            "",
            WS_TABSTOP | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32 | ES_WANTRETURN as u32,
        );
        SendMessageW(self.h(134), EM_SETLIMITTEXT, 8000, 0);
        for (id, label) in [
            (130, ""),
            (133, "回來先做什麼"),
            (150, ""),
            (202, "Google API key"),
            (204, "模型"),
            (211, ""),
            (
                213,
                "開啟後，新便條會送到 Google 分類。\r\n測試連線只送一則範例，不送你的便條。",
            ),
        ] {
            self.control(id, "STATIC", label, 0);
            SendMessageW(self.h(id), WM_SETFONT, self.tiny as usize, 1);
        }
        self.control(
            203,
            "EDIT",
            "",
            WS_TABSTOP | ES_PASSWORD as u32 | ES_AUTOHSCROLL as u32,
        );
        SendMessageW(self.h(203), EM_SETLIMITTEXT, 2000, 0);
        self.control(205, "EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL as u32);
        set(self.h(110), &self.board.draft);
        self.topmost();
        self.refresh();
        self.select(
            self.board
                .notes
                .iter()
                .rev()
                .find(|n| n.status != Status::Done)
                .map(|n| n.id),
        );
        self.hotkey = RegisterHotKey(
            self.hwnd,
            1,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            if std::env::var_os("RETURN_NOTE_DATA").is_some() {
                VK_F24 as u32
            } else {
                'N' as u32
            },
        ) != 0;
        if !self.hotkey {
            self.notice("快捷鍵已被占用，可從工作列叫出。");
        }
        SetTimer(self.hwnd, 1, 30_000, None);
        self.reminders();
        let tooltip = CreateWindowExW(
            WS_EX_TOPMOST,
            TOOLTIPS_CLASSW,
            null(),
            WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
            0,
            0,
            0,
            0,
            self.hwnd,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        for (id, label) in [
            (111, "新增便條 · Enter（Shift+Enter 換行）"),
            (112, "更多選項"),
            (113, "切換置頂"),
            (116, "返回便條"),
            (117, "收到工作列 · Ctrl+Alt+N 叫出"),
            (118, "收合／展開分類側欄"),
            (119, "依標籤排序"),
            (120, "拖曳排序 · 勾選完成 · 點標籤改分類"),
            (142, "變更這張便條的分類"),
        ] {
            self.tips.push(wide(label));
            let mut info = TTTOOLINFOW {
                cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
                uFlags: TTF_IDISHWND | TTF_SUBCLASS,
                hwnd: self.hwnd,
                uId: self.h(id) as usize,
                lpszText: self.tips.last_mut().unwrap().as_mut_ptr(),
                ..std::mem::zeroed()
            };
            SendMessageW(
                tooltip,
                TTM_ADDTOOLW,
                0,
                (&mut info as *mut TTTOOLINFOW) as isize,
            );
        }
        self.layout();
        SetFocus(self.h(110));
    }
    fn publish_view(&self) {
        let view = Visual {
            hwnd: self.hwnd,
            list: self.h(120),
            font: self.font,
            bold: self.bold,
            tiny: self.tiny,
            scale: self.scale,
            managing: self.managing,
            settings: self.settings,
            editing: self.editing,
            searching: self.searching,
            filter: self.filter,
            topmost: self.board.settings.topmost,
            sidebar_collapsed: self.board.settings.sidebar_collapsed,
            drag_source: self
                .drag
                .as_ref()
                .filter(|drag| drag.active)
                .map(|drag| drag.source),
            drag_indicator: self.drag.as_ref().and_then(|drag| drag.indicator),
            category_filter: self.category_filter,
            labels: self
                .board
                .settings
                .tags
                .iter()
                .map(|t| t.label.clone())
                .collect(),
            category_counts: self.category_counts(),
            selected_category: self
                .selected
                .and_then(|id| self.board.notes.iter().find(|n| n.id == id))
                .map(|n| self.board.index(&n.category))
                .unwrap_or(0),
            rows: self
                .rows
                .iter()
                .filter_map(|id| self.board.notes.iter().find(|n| n.id == *id))
                .map(|n| DisplayRow {
                    id: n.id,
                    title: n.title.clone(),
                    subtitle: Self::subtitle(n),
                    timing: n.timer_started.is_some(),
                    category: self.board.index(&n.category),
                    status: n.status,
                    reminder_due: n.reminder_due,
                })
                .collect(),
            toast: self.toast.clone(),
            toast_error: self.toast_error,
        };
        VIEW.with(|current| *current.borrow_mut() = Some(Rc::new(view)));
    }
    unsafe fn repaint_notice(&self) {
        self.publish_view();
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(self.hwnd, &mut r);
        r.top =
            (r.bottom - (if self.toast_error { 112.0 } else { 18.0 } * self.scale) as i32).max(0);
        InvalidateRect(self.hwnd, &r, 0);
    }
    unsafe fn pos(&self, id: i32, x: i32, y: i32, w: i32, h: i32) {
        let (x, y, w, h) = (
            (x as f64 * self.scale) as i32,
            (y as f64 * self.scale) as i32,
            (w as f64 * self.scale) as i32,
            (h as f64 * self.scale) as i32,
        );
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(self.h(id), &mut r);
        let mut point = POINT {
            x: r.left,
            y: r.top,
        };
        ScreenToClient(self.hwnd, &mut point);
        if (point.x, point.y, r.right - r.left, r.bottom - r.top) != (x, y, w, h) {
            MoveWindow(self.h(id), x, y, w, h, 0);
            InvalidateRect(self.h(id), null(), 0);
        }
    }
    unsafe fn layout(&self) {
        self.publish_view();
        let mut rect: RECT = std::mem::zeroed();
        GetClientRect(self.hwnd, &mut rect);
        let w = (rect.right as f64 / self.scale) as i32;
        let h = (rect.bottom as f64 / self.scale) as i32;
        let inner = w - 48;
        let mut shown = vec![117];
        self.pos(117, w - 40, 10, 28, 28);
        if self.managing {
            shown.extend([210, 220, 221, 222, 223, 224, 225, 226, 211]);
            self.pos(210, 16, 14, 28, 28);
            self.pos(225, 24, 71, inner, 22);
            self.pos(220, 24, 100, inner, 220);
            self.pos(226, 24, 156, inner, 22);
            self.pos(221, 34, 194, inner - 20, 24);
            for (i, id) in [222, 223, 224].iter().enumerate() {
                self.pos(*id, 24 + i as i32 * (inner / 3), 250, inner / 3 - 4, 36);
            }
            self.pos(211, 24, 315, inner, 90);
        } else if self.settings {
            shown.extend([
                210, 202, 203, 204, 205, 206, 207, 208, 209, 214, 211, 213, 215,
            ]);
            self.pos(210, 16, 14, 28, 28);
            self.pos(202, 24, 80, inner, 20);
            self.pos(203, 36, 112, inner - 24, 24);
            self.pos(204, 24, 158, inner, 20);
            self.pos(205, 36, 190, inner - 24, 24);
            self.pos(213, 24, 242, inner, 42);
            self.pos(206, 24, 296, (inner - 8) / 2, 36);
            self.pos(215, 32 + (inner - 8) / 2, 296, (inner - 8) / 2, 36);
            self.pos(211, 24, 344, inner, 62);
            self.pos(207, 24, 416, (inner - 8) / 2, 30);
            self.pos(208, 32 + (inner - 8) / 2, 416, (inner - 8) / 2, 30);
            self.pos(209, 24, 470, (inner - 8) / 2, 34);
            self.pos(214, 32 + (inner - 8) / 2, 470, (inner - 8) / 2, 34);
        } else if self.editing {
            shown.extend([116, 112, 131, 130, 133, 134, 138, 142]);
            self.pos(116, 16, 14, 28, 28);
            self.pos(112, w - 78, 14, 28, 28);
            self.pos(131, 24, 82, inner, 76);
            self.pos(142, 24, 164, 132, 28);
            self.pos(130, 172, 167, inner - 148, 22);
            self.pos(133, 24, 222, inner, 22);
            self.pos(134, 36, 258, inner - 24, 96);
            self.pos(138, 24, h - 66, inner, 38);
        } else {
            shown.extend([112, 113, 118, 119, 110, 111]);
            self.pos(118, 16, 10, 28, 28);
            set(
                self.h(118),
                if self.board.settings.sidebar_collapsed {
                    "展開分類"
                } else {
                    "收合分類"
                },
            );
            if !self.board.settings.sidebar_collapsed {
                let count = self.board.settings.tags.len() as i32 + 1;
                let overflow = count * 22 > h - 132;
                if overflow {
                    shown.push(333);
                    self.pos(333, 8, 56, 92, 220);
                    SendMessageW(
                        self.h(333),
                        CB_SETCURSEL,
                        self.category_filter.map_or(0, |i| i + 1),
                        0,
                    );
                }
                let category_pitch = ((h - 132) / count).clamp(22, 30);
                for index in 0..if overflow { 0 } else { count } {
                    shown.push(300 + index);
                    self.pos(
                        300 + index,
                        10,
                        56 + index * category_pitch,
                        88,
                        category_pitch - 2,
                    );
                }
            }
            let left = if self.board.settings.sidebar_collapsed {
                14
            } else {
                110
            };
            if !self.rows.is_empty() {
                shown.extend([120, 121]);
            }
            self.pos(112, w - 72, 10, 28, 28);
            self.pos(119, w - 136, 10, 28, 28);
            self.pos(113, w - 104, 10, 28, 28);
            let y = if self.searching { 100 } else { 56 };
            if self.searching {
                shown.push(115);
                self.pos(115, left + 10, 64, w - left - 40, 24);
            }
            let reserve = if self.toast_error { 52 } else { 0 };
            let list_height = (h - y - 76 - reserve).max(36);
            self.pos(120, left, y, w - left - 20, list_height);
            self.pos(121, w - 14, y, 8, list_height);
            self.measure_rows();
            self.pos(110, 26, h - 50, w - 88, 24);
            self.pos(111, w - 52, h - 54, 30, 30);
        }
        for (&id, &handle) in &self.controls {
            let visible = shown.contains(&id);
            if (IsWindowVisible(handle) != 0) != visible {
                ShowWindow(handle, if visible { SW_SHOWNA } else { SW_HIDE });
            }
        }
        InvalidateRect(self.hwnd, null(), 0);
    }

    unsafe fn notice(&mut self, s: &str) {
        let old_error = self.toast_error;
        self.toast_error = [
            "失敗",
            "無法",
            "找不到",
            "上限",
            "逾時",
            "不正確",
            "已暫停",
            "檢查",
            "占用",
            "用完",
            "拒絕",
            "錯誤",
        ]
        .iter()
        .any(|word| s.contains(word));
        self.toast = s.to_string();
        if self.settings {
            set(self.h(211), s);
        }
        if !self.toast_error {
            SetTimer(self.hwnd, 3, 2400, None);
        }
        if old_error != self.toast_error && !self.settings && !self.editing {
            self.layout();
        } else {
            self.repaint_notice();
        }
    }
    fn commit(&mut self, next: Board) -> Result<(), String> {
        self.storage.save(&next)?;
        self.board = next;
        Ok(())
    }
    unsafe fn save_edits(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        let before_revision = self
            .selected
            .and_then(|id| self.board.notes.iter().find(|n| n.id == id))
            .map(|n| n.revision);
        let mut next = self.board.clone();
        next.draft = text(self.h(110));
        if let Some(id) = self.selected {
            let category = SendMessageW(self.h(132), CB_GETCURSEL, 0, 0);
            next.edit(
                id,
                text(self.h(131)),
                text(self.h(134)),
                self.board
                    .settings
                    .tags
                    .get(category as usize)
                    .map_or("其他", |t| t.key.as_str())
                    .to_string(),
            )?;
        }
        self.commit(next)?;
        self.dirty = false;
        KillTimer(self.hwnd, 2);
        let after_revision = self
            .selected
            .and_then(|id| self.board.notes.iter().find(|n| n.id == id))
            .map(|n| n.revision);
        if before_revision != after_revision {
            self.refresh();
        }
        // Saving unfinished input is not adding a task. No success toast or list repaint.
        Ok(())
    }
    unsafe fn compact(&mut self, compact: bool) -> Result<(), String> {
        self.save_edits()?;
        self.remember_main_size();
        self.editing = !compact;
        self.settings = false;
        self.toast.clear();
        self.toast_error = false;
        self.resize();
        self.layout();
        Ok(())
    }
    unsafe fn remember_main_size(&mut self) {
        if !self.settings && !self.editing {
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(self.hwnd, &mut r);
            self.main_size = (
                (r.right as f64 / self.scale).round() as i32,
                (r.bottom as f64 / self.scale).round() as i32,
            );
        }
    }
    unsafe fn update_fonts(&mut self) {
        let make = |size, weight| make_font(size, weight, self.scale);
        let old = [self.font, self.bold, self.big, self.tiny];
        let fonts = [
            make(15.0, 400),
            make(14.0, 600),
            make(21.0, 500),
            make(12.0, 400),
        ];
        self.font = fonts[0];
        self.bold = fonts[1];
        self.big = fonts[2];
        self.tiny = fonts[3];
        self.publish_view();
        for (&id, &h) in &self.controls {
            let f = if id == 131 {
                self.big
            } else if [130, 133, 202, 204, 211, 213].contains(&id) {
                self.tiny
            } else {
                self.font
            };
            SendMessageW(h, WM_SETFONT, f as usize, 1);
        }
        self.measure_rows();
        for f in old {
            DeleteObject(f);
        }
    }
    unsafe fn resize(&self) {
        let (w, h) = if self.settings {
            (380, 560)
        } else if self.editing {
            (344, 460)
        } else {
            self.main_size
        };
        SetWindowPos(
            self.hwnd,
            null_mut(),
            0,
            0,
            (w as f64 * self.scale) as i32,
            (h as f64 * self.scale) as i32,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    unsafe fn select(&mut self, id: Option<u64>) {
        self.selected = id;
        if let Some(n) = id.and_then(|id| self.board.notes.iter().find(|n| n.id == id)) {
            set(self.h(131), &n.title);
            set(self.h(134), &n.resume);
            SendMessageW(self.h(132), CB_SETCURSEL, self.board.index(&n.category), 0);
            set(self.h(130), n.status.label());
            set(
                self.h(142),
                &format!(
                    "分類：{}",
                    self.board.settings.tags[self.board.index(&n.category)].label
                ),
            );
            set(
                self.h(138),
                if n.status == Status::Done {
                    "恢復待辦"
                } else {
                    "完成"
                },
            );
            set(
                self.h(139),
                if n.remind_at.is_some() {
                    "取消回看提醒"
                } else {
                    "10 分後回看"
                },
            );
            EnableWindow(self.h(139), (n.status == Status::Waiting) as i32);
        } else {
            set(self.h(131), "");
            set(self.h(134), "");
            set(self.h(130), "選一張便條，可以留下回來的第一步。");
        }
        for id in [131, 132, 134, 135, 136, 137, 138, 140, 141, 142] {
            EnableWindow(self.h(id), self.selected.is_some() as i32);
        }
        if self.selected.is_none() {
            EnableWindow(self.h(139), 0);
        }
        self.publish_view();
        InvalidateRect(self.h(142), null(), 0);
        if let Some(index) = self.rows.iter().position(|id| Some(*id) == self.selected)
            && SendMessageW(self.h(120), LB_GETCURSEL, 0, 0) != index as isize
        {
            SendMessageW(self.h(120), LB_SETCURSEL, index, 0);
        }
    }
    unsafe fn sync_tags(&self) {
        for id in [132, 220, 333] {
            SendMessageW(self.h(id), CB_RESETCONTENT, 0, 0);
            if id == 333 {
                SendMessageW(self.h(id), CB_ADDSTRING, 0, wide("全部").as_ptr() as isize);
            }
            for t in &self.board.settings.tags {
                SendMessageW(
                    self.h(id),
                    CB_ADDSTRING,
                    0,
                    wide(&t.label).as_ptr() as isize,
                );
            }
        }
    }
    fn category_counts(&self) -> Vec<usize> {
        let mut counts = vec![0; self.board.settings.tags.len() + 1];
        for n in &self.board.notes {
            if (self.filter == 4) == (n.status == Status::Done) {
                counts[0] += 1;
                counts[self.board.index(&n.category) + 1] += 1;
            }
        }
        counts
    }
    unsafe fn refresh(&mut self) {
        self.cancel_drag();
        count_event(2);
        let search = text(self.h(115)).to_lowercase();
        self.rows = self
            .board
            .ordered_ids()
            .into_iter()
            .filter_map(|id| self.board.notes.iter().find(|note| note.id == id))
            .filter(|n| match self.filter {
                1 => n.status == Status::Now,
                2 => n.status == Status::Waiting,
                3 => n.status == Status::Later,
                4 => n.status == Status::Done,
                _ => n.status != Status::Done,
            })
            .filter(|n| {
                self.category_filter
                    .is_none_or(|index| n.category == self.board.settings.tags[index].key)
            })
            .filter(|n| {
                search.is_empty()
                    || format!(
                        "{} {} {} {} {}",
                        n.title,
                        n.original,
                        n.resume,
                        n.category,
                        self.board.settings.tags[self.board.index(&n.category)].label
                    )
                    .to_lowercase()
                    .contains(&search)
            })
            .map(|n| n.id)
            .collect();
        KillTimer(self.hwnd, 5);
        if self
            .board
            .notes
            .iter()
            .any(|n| n.timer_started.is_some() && self.rows.contains(&n.id))
        {
            SetTimer(self.hwnd, 5, 1000, None);
        }
        self.publish_view();
        let counts = self.category_counts();
        set(self.h(300), &format!("全部分類，共 {} 筆", counts[0]));
        for (index, category) in self
            .board
            .settings
            .tags
            .iter()
            .map(|t| &t.label)
            .enumerate()
        {
            set(
                self.h(301 + index as i32),
                &format!("{}，{} 筆", category, counts[index + 1]),
            );
        }
        SendMessageW(self.h(120), WM_SETREDRAW, 0, 0);
        SendMessageW(self.h(120), LB_RESETCONTENT, 0, 0);
        for id in &self.rows {
            let n = self.board.notes.iter().find(|n| n.id == *id).unwrap();
            let label = format!(
                "{} · {} · {}",
                n.status.label(),
                self.board.settings.tags[self.board.index(&n.category)].label,
                n.title
            );
            SendMessageW(self.h(120), LB_ADDSTRING, 0, wide(&label).as_ptr() as isize);
        }
        self.measure_rows();
        if let Some(i) = self.rows.iter().position(|id| Some(*id) == self.selected) {
            SendMessageW(self.h(120), LB_SETCURSEL, i, 0);
        }
        SendMessageW(self.h(120), WM_SETREDRAW, 1, 0);
        InvalidateRect(self.h(120), null(), 0);
        self.layout();
        for id in 300..=332 {
            InvalidateRect(self.h(id), null(), 0);
        }
    }

    fn subtitle(n: &return_note::model::Note) -> String {
        let status = if n.reminder_due {
            "該回看了"
        } else {
            match n.status {
                Status::Now => "現在做",
                Status::Waiting => "等結果",
                Status::Later => "稍後",
                _ => "",
            }
        };
        let detail =
            if matches!(n.status, Status::Now | Status::Waiting) && !n.resume.trim().is_empty() {
                format!("{} · {}", status, n.resume.replace(['\r', '\n'], " "))
            } else {
                status.to_string()
            };
        if n.timer_started.is_none() && n.spent_seconds.is_none() {
            return detail;
        }
        let t = n.elapsed(now());
        let prefix = if n.timer_started.is_some() {
            "計時中"
        } else {
            "用時"
        };
        let duration = format!("{prefix} {}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60);
        if detail.is_empty() {
            duration
        } else {
            format!("{duration} · {detail}")
        }
    }
    unsafe fn measure_rows(&self) {
        let list = self.h(120);
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(list, &mut r);
        let dc = GetDC(list);
        let old = SelectObject(dc, self.font);
        for (index, id) in self.rows.iter().enumerate() {
            if let Some(n) = self.board.notes.iter().find(|n| n.id == *id) {
                let mut bounds = RECT {
                    right: (r.right - (126.0 * self.scale) as i32).max(40),
                    ..Default::default()
                };
                DrawTextW(
                    dc,
                    wide(&n.title).as_ptr(),
                    -1,
                    &mut bounds,
                    DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX,
                );
                let title_height =
                    ((bounds.bottom as f64 / self.scale).ceil() as i32).clamp(20, 42);
                let height = title_height + 12 + if Self::subtitle(n).is_empty() { 0 } else { 20 };
                let pixels = (height as f64 * self.scale).round() as isize;
                if SendMessageW(list, LB_GETITEMHEIGHT, index, 0) != pixels {
                    SendMessageW(list, LB_SETITEMHEIGHT, index, pixels);
                }
            }
        }
        SelectObject(dc, old);
        ReleaseDC(list, dc);
        InvalidateRect(self.h(121), null(), 1);
    }
    unsafe fn cancel_drag(&mut self) {
        if self.drag.take().is_none() {
            return;
        }
        KillTimer(self.hwnd, 4);
        RemovePropW(self.h(120), wide("rn.dragging").as_ptr());
        if GetCapture() == self.h(120) {
            ReleaseCapture();
        }
        self.publish_view();
        InvalidateRect(self.h(120), null(), 0);
    }
    unsafe fn begin_drag(&mut self, index: usize, point: POINT) -> Result<bool, String> {
        self.save_edits()?;
        let Some(&source) = self.rows.get(index) else {
            return Ok(false);
        };
        self.drag = Some(DragState {
            source,
            rows: self.rows.clone(),
            start: point,
            point,
            active: false,
            slot: None,
            indicator: None,
            scroll_direction: 0,
        });
        Ok(true)
    }
    unsafe fn update_drag(&mut self, point: POINT, scroll: bool) -> bool {
        let Some(mut drag) = self.drag.take() else {
            return false;
        };
        let previous_visual = (drag.active, drag.indicator);
        drag.point = point;
        if !drag.active
            && ((point.x - drag.start.x).abs() >= GetSystemMetrics(SM_CXDRAG).max(4)
                || (point.y - drag.start.y).abs() >= GetSystemMetrics(SM_CYDRAG).max(4))
        {
            drag.active = true;
        }
        let mut bounds: RECT = std::mem::zeroed();
        GetClientRect(self.h(120), &mut bounds);
        let inside =
            point.x >= 0 && point.x < bounds.right && point.y >= 0 && point.y < bounds.bottom;
        let list = self.h(120);
        let top = SendMessageW(list, LB_GETTOPINDEX, 0, 0);
        let edge = (18.0 * self.scale) as i32;
        let mut direction = 0;
        if drag.active && inside && !self.rows.is_empty() {
            if point.y < edge && top > 0 {
                direction = -1;
            } else if point.y > bounds.bottom - edge {
                let mut last: RECT = std::mem::zeroed();
                SendMessageW(
                    list,
                    LB_GETITEMRECT,
                    self.rows.len() - 1,
                    (&mut last as *mut RECT) as isize,
                );
                if last.bottom > bounds.bottom {
                    direction = 1;
                }
            }
        }
        // The timer exists only while an edge can actually scroll. Resetting it on
        // every mouse move delays scrolling; running it in the middle repaints forever.
        if direction != drag.scroll_direction {
            KillTimer(self.hwnd, 4);
            if direction != 0 {
                SetTimer(self.hwnd, 4, 100, None);
            }
            drag.scroll_direction = direction;
        }
        if scroll && direction != 0 {
            SendMessageW(list, LB_SETTOPINDEX, (top + direction as isize) as usize, 0);
        }
        let scrolled = SendMessageW(list, LB_GETTOPINDEX, 0, 0) != top;
        drag.slot = None;
        drag.indicator = None;
        if drag.active && inside {
            let mut slot = self.rows.len();
            let mut line = 0;
            for index in 0..self.rows.len() {
                let mut rect: RECT = std::mem::zeroed();
                SendMessageW(
                    self.h(120),
                    LB_GETITEMRECT,
                    index,
                    (&mut rect as *mut RECT) as isize,
                );
                line = rect.bottom;
                if point.y < (rect.top + rect.bottom) / 2 {
                    slot = index;
                    line = rect.top;
                    break;
                }
            }
            drag.slot = Some(slot);
            drag.indicator = Some(line.clamp(2, (bounds.bottom - 2).max(2)));
        }
        let active = drag.active;
        let changed = previous_visual != (active, drag.indicator) || scrolled;
        self.drag = Some(drag);
        if changed {
            self.publish_view();
            InvalidateRect(list, null(), 0);
            if scrolled {
                InvalidateRect(self.h(121), null(), 0);
            }
        }
        active
    }
    unsafe fn finish_drag(&mut self, point: POINT) -> Result<(), String> {
        self.update_drag(point, false);
        let Some(drag) = self.drag.take() else {
            return Ok(());
        };
        KillTimer(self.hwnd, 4);
        RemovePropW(self.h(120), wide("rn.dragging").as_ptr());
        if GetCapture() == self.h(120) {
            ReleaseCapture();
        }
        self.publish_view();
        InvalidateRect(self.h(120), null(), 0);
        if !drag.active
            && drag.start.x < (14.0 * self.scale) as i32
            && point.x >= 0
            && point.x < (14.0 * self.scale) as i32
            && point.y >= 0
        {
            let mut next = self.board.clone();
            if let Some(note) = next
                .notes
                .iter_mut()
                .find(|n| n.id == drag.source && n.status != Status::Done)
            {
                note.toggle_timer(now());
                self.commit(next)?;
                self.refresh();
            }
            return Ok(());
        }
        if !drag.active || drag.slot.is_none() {
            return Ok(());
        }
        if self.rows != drag.rows {
            return Err("清單已更新，請再拖一次。".into());
        }
        let before = self.rows.get(drag.slot.unwrap()).copied();
        self.reorder_note(drag.source, before)
    }
    unsafe fn reorder_note(&mut self, source: u64, before: Option<u64>) -> Result<(), String> {
        let mut next = self.board.clone();
        if next.reorder_visible(&self.rows, source, before)? {
            self.commit(next)?;
            self.refresh();
            self.select(Some(source));
            self.notice("順序已儲存");
        }
        Ok(())
    }
    unsafe fn move_selected(&mut self, down: bool) -> Result<(), String> {
        self.save_edits()?;
        let Some(index) = self.rows.iter().position(|id| Some(*id) == self.selected) else {
            return Ok(());
        };
        let source = self.rows[index];
        if down {
            if index + 1 < self.rows.len() {
                self.reorder_note(source, self.rows.get(index + 2).copied())?;
            }
        } else if index > 0 {
            self.reorder_note(source, Some(self.rows[index - 1]))?;
        }
        Ok(())
    }
    unsafe fn capture(&mut self) -> Result<(), String> {
        self.save_edits()?;
        let raw = text(self.h(110));
        let mut next = self.board.clone();
        let id = next.capture(&raw)?;
        let fixed_category = self
            .category_filter
            .map(|index| self.board.settings.tags[index].key.clone());
        if let Some(category) = &fixed_category {
            let note = next.notes.last_mut().unwrap();
            note.category.clone_from(category);
            note.ai = "指定分類".into();
        }
        self.commit(next)?;
        set(self.h(110), "");
        self.filter = 0;
        SendMessageW(self.h(114), CB_SETCURSEL, 0, 0);
        set(self.h(115), "");
        self.refresh();
        self.select(Some(id));
        if let Some(index) = self.rows.iter().position(|row| *row == id) {
            SendMessageW(self.h(120), LB_SETTOPINDEX, index, 0);
        }
        self.notice("已加入清單");
        SetFocus(self.h(110));
        self.enqueue(id, fixed_category);
        Ok(())
    }
    unsafe fn enqueue(&mut self, id: u64, fixed_category: Option<String>) {
        if self.in_flight == Some(id) || self.queue.iter().any(|job| job.id == id) {
            self.notice("這張便條正在分類，不會重複送出。");
            return;
        }
        if !self.board.settings.ai_enabled {
            return;
        }
        if self.ai_paused {
            self.notice("AI 已暫停。請檢查設定，再按重新分類。");
            return;
        }
        if self.queue.len() >= 5 {
            self.notice("已存好。分類工作較多，這張先用本機分類。");
            return;
        }
        if let Some(n) = self.board.notes.iter().find(|n| n.id == id) {
            self.queue.push_back(Job {
                id,
                revision: n.revision,
                original: n.original.clone(),
                fixed_category,
            });
        }
        self.start_job();
    }
    unsafe fn start_job(&mut self) {
        if self.busy || !self.board.settings.ai_enabled || self.ai_paused {
            return;
        }
        let Some(job) = self.queue.pop_front() else {
            return;
        };
        let key = match network::read_key() {
            Ok(Some(key)) => key,
            _ => {
                self.ai_paused = true;
                self.queue.clear();
                self.notice("找不到可用的 Google API key，便條已存好。");
                return;
            }
        };
        let model = self.board.settings.model.clone();
        let tx = self.tx.clone();
        let hwnd = self.hwnd as usize;
        self.busy = true;
        self.in_flight = Some(job.id);
        if job.id == 0 {
            set(self.h(215), "測試中…");
            EnableWindow(self.h(215), 0);
            self.ai_status = "正在測試 Google 連線…".into();
            set(self.h(211), &self.ai_status);
        } else {
            self.notice("已加入清單，正在分類…");
        }
        let tags = self.board.settings.tags.clone();
        std::thread::spawn(move || {
            let labels = tags.iter().map(|t| t.label.clone()).collect::<Vec<_>>();
            let result = network::classify(&job.original, &model, &key, &labels).map(
                |mut classification| {
                    if let Some(t) = tags.iter().find(|t| t.label == classification.category) {
                        classification.category = t.key.clone();
                    }
                    if let Some(category) = job.fixed_category {
                        classification.category = category;
                    }
                    classification
                },
            );
            let _ = tx.send(Outcome {
                id: job.id,
                revision: job.revision,
                result,
            });
            unsafe {
                PostMessageW(hwnd as HWND, RESULT, 0, 0);
            }
        });
    }
    unsafe fn results(&mut self) -> Result<(), String> {
        // Commit edits already visible in the controls before comparing revisions.
        self.save_edits()?;
        while let Ok(out) = self.rx.try_recv() {
            self.busy = false;
            self.in_flight = None;
            set(self.h(215), "測試連線");
            EnableWindow(self.h(215), 1);
            if !self.board.settings.ai_enabled {
                continue;
            }
            match out.result {
                Ok(c) => {
                    self.ai_status = "連線成功，AI 分類可使用。".into();
                    if self.settings {
                        set(self.h(211), &self.ai_status);
                    }
                    if out.id == 0 {
                        self.ai_paused = false;
                        if !self.settings {
                            self.notice("連線成功，AI 分類可使用。");
                        }
                        continue;
                    }
                    let category = c.category.clone();
                    let mut next = self.board.clone();
                    let changed = next.apply_ai(out.id, out.revision, c.title, c.category);
                    if changed {
                        self.commit(next)?;
                        self.refresh();
                        if !self.dirty {
                            if !self.editing
                                && !self.rows.iter().any(|id| Some(*id) == self.selected)
                            {
                                self.select(self.rows.first().copied());
                            } else {
                                self.select(self.selected);
                            }
                        }
                        self.notice(&format!(
                            "已分到{}",
                            self.board.settings.tags[self.board.index(&category)].label
                        ));
                    } else {
                        self.notice("已保留你的修改");
                    }
                }
                Err(e) => {
                    self.ai_status = e.clone();
                    self.ai_paused = true;
                    self.queue.clear();
                    self.notice(&e);
                }
            }
        }
        self.start_job();
        Ok(())
    }
    unsafe fn topmost(&self) {
        self.publish_view();
        SetWindowPos(
            self.hwnd,
            if self.board.settings.topmost {
                HWND_TOPMOST
            } else {
                HWND_NOTOPMOST
            },
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        set(
            self.h(113),
            if self.board.settings.topmost {
                "置頂中"
            } else {
                "置頂"
            },
        );
    }
    unsafe fn settings(&mut self, show: bool) -> Result<(), String> {
        self.save_edits()?;
        self.remember_main_size();
        self.managing = false;
        self.settings = show;
        self.editing = false;
        self.toast.clear();
        self.toast_error = false;
        if show {
            set(self.h(203), "");
            set(self.h(205), &self.board.settings.model);
            self.update_ai_status();
        }
        self.resize();
        self.layout();
        SetFocus(if show { self.h(203) } else { self.h(110) });
        Ok(())
    }
    unsafe fn update_ai_status(&self) {
        let key = matches!(network::read_key(), Ok(Some(_)));
        let label = if !key {
            "尚未設定 Google API key。"
        } else if !self.board.settings.ai_enabled {
            "金鑰已保存，目前使用本機分類。"
        } else if self.ai_status.is_empty() {
            "金鑰已保存。可按「測試連線」確認額度與模型。"
        } else {
            &self.ai_status
        };
        set(self.h(211), label);
    }
    unsafe fn save_ai_settings(&mut self) -> Result<(), String> {
        if self.busy {
            return Err("請等目前的 AI 請求完成，再變更設定。".into());
        }
        let model = text(self.h(205)).trim().to_string();
        if !return_note::ai::valid_model(&model) {
            return Err("模型名稱請使用英數字、連字號與小數點。".into());
        }
        let key = text(self.h(203));
        if !key.trim().is_empty() {
            network::save_key(&key)?;
            set(self.h(203), "");
        }
        if network::read_key()?.is_none() {
            return Err("請先填入 Google API key。".into());
        }
        let mut next = self.board.clone();
        next.settings.ai_enabled = true;
        next.settings.model = model;
        self.commit(next)?;
        self.ai_paused = false;
        self.ai_status.clear();
        self.update_ai_status();
        Ok(())
    }
    unsafe fn category_menu(&mut self) -> Result<(), String> {
        self.save_edits()?;
        let Some(note) = self
            .selected
            .and_then(|id| self.board.notes.iter().find(|n| n.id == id))
        else {
            return Ok(());
        };
        let menu = CreatePopupMenu();
        for (index, category) in self.board.settings.tags.iter().map(|t| &t.key).enumerate() {
            AppendMenuW(
                menu,
                MF_STRING
                    | if note.category == *category {
                        MF_CHECKED
                    } else {
                        0
                    },
                1200 + index,
                wide(&self.board.settings.tags[index].label).as_ptr(),
            );
        }
        let mut point: POINT = std::mem::zeroed();
        GetCursorPos(&mut point);
        let command = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            self.hwnd,
            null(),
        ) as i32;
        DestroyMenu(menu);
        if command != 0 {
            self.action(command, BN_CLICKED)?;
        }
        Ok(())
    }
    unsafe fn menu(&mut self, note: bool) -> Result<(), String> {
        self.save_edits()?;
        let menu = CreatePopupMenu();
        let add = |id: usize, label: &str| {
            AppendMenuW(menu, MF_STRING, id, wide(label).as_ptr());
        };
        if note && self.selected.is_some() {
            add(116, "編輯便條");
            add(135, "現在做");
            add(136, "等結果");
            add(137, "稍後");
            add(138, "完成 / 恢復");
            add(940, "上移\tAlt+↑");
            add(941, "下移\tAlt+↓");
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            add(139, "10 分後回看 / 取消提醒");
            add(140, "看原文");
            add(141, "重新分類");
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            for (i, label) in self
                .board
                .settings
                .tags
                .iter()
                .map(|t| &t.label)
                .enumerate()
            {
                add(1200 + i, &format!("分類：{label}"));
            }
        } else {
            add(904, "搜尋便條\tCtrl+F");
            add(
                905,
                if self.filter == 4 {
                    "回到未完成"
                } else {
                    "已完成的便條"
                },
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            let sizes = CreatePopupMenu();
            for (i, percent) in ZOOM_LEVELS.iter().enumerate() {
                let flags = if *percent == self.board.settings.zoom_percent {
                    MF_CHECKED
                } else {
                    MF_STRING
                };
                AppendMenuW(sizes, flags, 950 + i, wide(&format!("{percent}%")).as_ptr());
            }
            AppendMenuW(
                menu,
                MF_POPUP,
                sizes as usize,
                wide("字體與便條大小").as_ptr(),
            );
            add(906, "管理標籤");
            add(900, "設定");
            add(212, "結束程式");
        }
        let mut pos: POINT = std::mem::zeroed();
        GetCursorPos(&mut pos);
        let command = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            pos.x,
            pos.y,
            self.hwnd,
            null(),
        ) as i32;
        DestroyMenu(menu);
        if command != 0 {
            self.action(command, BN_CLICKED)?;
        }
        Ok(())
    }
    unsafe fn reminders(&mut self) {
        let mut next = self.board.clone();
        if next.due_reminders(now()) {
            match self.commit(next) {
                Ok(()) => {
                    self.refresh();
                    self.notice("有一件等結果的工作，可以回去看一下了。");
                    let f = FLASHWINFO {
                        cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
                        hwnd: self.hwnd,
                        dwFlags: FLASHW_TRAY,
                        uCount: 3,
                        dwTimeout: 0,
                    };
                    FlashWindowEx(&f);
                }
                Err(e) => self.notice(&e),
            }
        }
    }
    unsafe fn action(&mut self, id: i32, event: u32) -> Result<(), String> {
        if self.drag.is_some() {
            self.cancel_drag();
        }
        if [110, 131, 134].contains(&id) && event == EN_CHANGE {
            self.dirty = true;
            InvalidateRect(self.h(110), null(), 0);
            SetTimer(self.hwnd, 2, 450, None);
            return Ok(());
        }
        if id == 333 && event == CBN_SELCHANGE {
            let ix = SendMessageW(self.h(333), CB_GETCURSEL, 0, 0);
            if ix >= 0 {
                self.action(300 + ix as i32, BN_CLICKED)?;
            }
            return Ok(());
        }
        if id == 220 && event == CBN_SELCHANGE {
            let ix = SendMessageW(self.h(220), CB_GETCURSEL, 0, 0) as usize;
            if let Some(t) = self.board.settings.tags.get(ix) {
                set(self.h(221), &t.label);
            }
            return Ok(());
        }
        if id == 132 && event == CBN_SELCHANGE {
            self.dirty = true;
            InvalidateRect(self.h(110), null(), 0);
            SetTimer(self.hwnd, 2, 450, None);
            return Ok(());
        }
        if id == 115 && event == EN_CHANGE {
            self.save_edits()?;
            self.refresh();
            return Ok(());
        }
        if id == 114 && event == CBN_SELCHANGE {
            self.save_edits()?;
            self.filter = SendMessageW(self.h(114), CB_GETCURSEL, 0, 0) as usize;
            self.refresh();
            self.select(self.rows.first().copied());
            return Ok(());
        }
        if id == 120 && event == LBN_SELCHANGE {
            let row = SendMessageW(self.h(120), LB_GETCURSEL, 0, 0);
            let id = self.rows.get(row as usize).copied();
            self.save_edits()?;
            self.select(id);
            return Ok(());
        }
        if id == 120 && event == LBN_DBLCLK {
            self.compact(false)?;
            SetFocus(self.h(134));
            return Ok(());
        }
        if event != BN_CLICKED {
            return Ok(());
        }
        match id {
            950..=954 => {
                let percent = ZOOM_LEVELS[(id - 950) as usize];
                self.save_edits()?;
                self.remember_main_size();
                let mut next = self.board.clone();
                next.settings.zoom_percent = percent;
                self.commit(next)?;
                self.scale = GetDpiForWindow(self.hwnd) as f64 / 96.0 * f64::from(percent) / 100.0;
                self.update_fonts();
                self.resize();
                self.layout();
            }
            111 => self.capture()?,
            118 => {
                self.save_edits()?;
                let mut next = self.board.clone();
                next.settings.sidebar_collapsed = !next.settings.sidebar_collapsed;
                self.commit(next)?;
                self.layout();
            }
            119 => {
                self.save_edits()?;
                let mut next = self.board.clone();
                if next.sort_by_tag(&self.rows)? {
                    self.commit(next)?;
                    self.refresh();
                    SendMessageW(self.h(120), LB_SETTOPINDEX, 0, 0);
                    self.notice("已依標籤排序");
                } else {
                    self.notice("目前已依標籤排列");
                }
            }
            940 | 941 => self.move_selected(id == 941)?,
            142 | 930 => self.category_menu()?,
            300..=332 => {
                self.save_edits()?;
                self.category_filter = (id > 300).then_some((id - 301) as usize);
                self.refresh();
                self.select(self.rows.first().copied());
            }
            112 => self.menu(self.editing)?,
            117 => {
                self.save_edits()?;
                ShowWindow(self.hwnd, SW_MINIMIZE);
            }
            906 => {
                self.settings(true)?;
                self.managing = true;
                self.layout();
                SendMessageW(self.h(220), CB_SETCURSEL, 0, 0);
                set(self.h(221), "");
                set(
                    self.h(211),
                    "輸入名稱可新增標籤。選取既有標籤可改名或刪除。",
                );
                SetFocus(self.h(221));
            }
            222..=224 if self.managing => {
                let ix = SendMessageW(self.h(220), CB_GETCURSEL, 0, 0) as usize;
                let name = text(self.h(221));
                let mut next = self.board.clone();
                next.change_tag(
                    (id != 222).then_some(ix),
                    (id != 224).then_some(name.as_str()),
                )?;
                if id == 224
                    && MessageBoxW(
                        self.hwnd,
                        wide("刪除這個標籤？使用它的便條會移到「其他」。").as_ptr(),
                        wide("刪除標籤").as_ptr(),
                        MB_YESNO | MB_DEFBUTTON2,
                    ) != IDYES
                {
                    return Ok(());
                }
                let filter = self
                    .category_filter
                    .map(|i| self.board.settings.tags[i].key.clone());
                self.commit(next)?;
                self.category_filter = filter
                    .and_then(|key| self.board.settings.tags.iter().position(|t| t.key == key));
                self.sync_tags();
                self.refresh();
                self.select(self.rows.first().copied());
                let i = if id == 222 {
                    self.board.settings.tags.len() - 2
                } else {
                    ix.min(self.board.settings.tags.len() - 1)
                };
                SendMessageW(self.h(220), CB_SETCURSEL, i, 0);
                set(self.h(221), &self.board.settings.tags[i].label);
                set(self.h(211), "標籤已儲存。");
            }
            900 => self.settings(true)?,
            904 => {
                self.save_edits()?;
                self.searching = !self.searching;
                self.editing = false;
                self.settings = false;
                if !self.searching {
                    set(self.h(115), "");
                    self.refresh();
                }
                self.resize();
                self.layout();
                SetFocus(if self.searching {
                    self.h(115)
                } else {
                    self.h(110)
                });
            }
            905 => {
                self.save_edits()?;
                self.filter = if self.filter == 4 { 0 } else { 4 };
                self.refresh();
                self.select(self.rows.first().copied());
            }
            1200..=1231 => {
                SendMessageW(self.h(132), CB_SETCURSEL, (id - 1200) as usize, 0);
                self.dirty = true;
                self.save_edits()?;
                let category = self.board.settings.tags[(id - 1200) as usize].key.clone();
                if self
                    .category_filter
                    .is_some_and(|index| self.board.settings.tags[index].key != category)
                    && !self.editing
                {
                    self.select(self.rows.first().copied());
                } else {
                    self.select(self.selected);
                }
                self.notice(&format!(
                    "已移至{}",
                    self.board.settings.tags[self.board.index(&category)].label
                ));
            }
            116 => self.compact(self.editing)?,
            113 => {
                let mut next = self.board.clone();
                next.settings.topmost = !next.settings.topmost;
                self.commit(next)?;
                self.topmost();
            }
            135..=138 => {
                self.save_edits()?;
                if let Some(note_id) = self.selected {
                    let mut next = self.board.clone();
                    let done = next
                        .notes
                        .iter()
                        .find(|n| n.id == note_id)
                        .is_some_and(|n| n.status == Status::Done);
                    let status = match id {
                        135 => Status::Now,
                        136 => Status::Waiting,
                        137 => Status::Later,
                        _ => {
                            if done {
                                Status::Inbox
                            } else {
                                Status::Done
                            }
                        }
                    };
                    let nearby = if id == 138 && !self.editing {
                        self.rows
                            .iter()
                            .position(|row| *row == note_id)
                            .map(|index| {
                                let top =
                                    SendMessageW(self.h(120), LB_GETTOPINDEX, 0, 0).max(0) as usize;
                                (index.saturating_sub(1), top)
                            })
                    } else {
                        None
                    };
                    next.set_status(note_id, status)?;
                    self.commit(next)?;
                    self.refresh();
                    if let Some((index, top)) = nearby {
                        self.select(self.rows.get(index).copied());
                        if !self.rows.is_empty() {
                            SendMessageW(self.h(120), LB_SETTOPINDEX, top.min(index), 0);
                            InvalidateRect(self.h(121), null(), 0);
                        }
                    } else {
                        self.select(Some(note_id));
                    }
                    self.notice(if status == Status::Waiting {
                        "等結果時，留一句回來要看什麼。"
                    } else {
                        "已存好。"
                    });
                    if status == Status::Waiting {
                        SetFocus(self.h(134));
                    }
                }
            }
            139 => {
                self.save_edits()?;
                if let Some(id) = self.selected {
                    let mut next = self.board.clone();
                    let n = next.notes.iter_mut().find(|n| n.id == id).unwrap();
                    if n.status == Status::Waiting {
                        n.remind_at = if n.remind_at.is_some() {
                            None
                        } else {
                            Some(now() + 600)
                        };
                        n.reminder_due = false;
                        self.commit(next)?;
                        self.select(Some(id));
                        self.refresh();
                        self.notice("回看提醒已更新");
                    }
                }
            }
            140 => {
                if let Some(n) = self
                    .selected
                    .and_then(|id| self.board.notes.iter().find(|n| n.id == id))
                {
                    message(self.hwnd, &n.original);
                }
            }
            141 => {
                self.save_edits()?;
                self.ai_paused = false;
                if let Some(id) = self.selected {
                    self.enqueue(id, None);
                }
            }
            206 => {
                self.save_ai_settings()?;
                self.toast.clear();
                self.toast_error = false;
                self.layout();
            }
            215 => {
                self.save_ai_settings()?;
                self.queue.push_front(Job {
                    id: 0,
                    revision: 0,
                    original: "明天回覆客戶的合作報價".into(),
                    fixed_category: None,
                });
                self.start_job();
            }
            207 => {
                let mut next = self.board.clone();
                next.settings.ai_enabled = false;
                self.commit(next)?;
                self.queue.clear();
                self.settings(false)?;
                self.notice("已改用本機分類。");
            }
            208 => {
                self.ai_status.clear();
                network::delete_key()?;
                let mut next = self.board.clone();
                next.settings.ai_enabled = false;
                self.commit(next)?;
                self.queue.clear();
                self.settings(true)?;
                self.notice("金鑰已移除。");
            }
            209 => {
                self.save_edits()?;
                let p = self.storage.export(&self.board)?;
                message(self.hwnd, &format!("已匯出到：\r\n{}", p.display()));
            }
            210 => self.settings(false)?,
            214 => {
                let dir = self.storage.path.parent().ok_or("找不到資料夾。")?;
                let result = windows_sys::Win32::UI::Shell::ShellExecuteW(
                    self.hwnd,
                    wide("open").as_ptr(),
                    wide(&dir.to_string_lossy()).as_ptr(),
                    null(),
                    null(),
                    SW_SHOWNORMAL,
                );
                if result as isize <= 32 {
                    return Err("無法開啟資料夾。".into());
                }
            }
            212 => {
                self.save_edits()?;
                DestroyWindow(self.hwnd);
            }
            _ => {}
        }
        Ok(())
    }
}

// Verify the selected face; a missing preferred face must not silently map to a serif.
unsafe fn make_font(size: f64, weight: i32, scale: f64) -> HFONT {
    let dc = GetDC(null_mut());
    for face in ["Noto Sans TC", "Microsoft JhengHei UI", "Segoe UI"] {
        let font = CreateFontW(
            -(size * scale).round() as i32,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            DEFAULT_PITCH as u32,
            wide(face).as_ptr(),
        );
        let old = SelectObject(dc, font);
        let mut selected = [0u16; 64];
        let len = GetTextFaceW(dc, selected.len() as i32, selected.as_mut_ptr());
        SelectObject(dc, old);
        if String::from_utf16_lossy(&selected[..len.max(0) as usize]).trim_end_matches('\0') == face
            || face == "Segoe UI"
        {
            ReleaseDC(null_mut(), dc);
            return font;
        }
        DeleteObject(font);
    }
    unreachable!()
}

// A narrow, draggable rail keeps the native list's keyboard/accessibility support.
unsafe fn scroll_geometry(list: HWND) -> (i32, i32, i32, i32) {
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(list, &mut r);
    let count = SendMessageW(list, LB_GETCOUNT, 0, 0).max(0) as usize;
    let top = SendMessageW(list, LB_GETTOPINDEX, 0, 0).max(0) as usize;
    let mut total = 0;
    let mut offset = 0;
    for i in 0..count {
        let h = SendMessageW(list, LB_GETITEMHEIGHT, i, 0).max(1) as i32;
        total += h;
        if i < top {
            offset += h;
        }
    }
    let view = r.bottom.max(1);
    let thumb = if total > view {
        (view * view / total).max(24).min(view)
    } else {
        view
    };
    let y = if total > view {
        offset * (view - thumb) / (total - view)
    } else {
        0
    };
    (view, total, thumb, y.min(view - thumb))
}
unsafe fn drag_scroll(rail: HWND, y: i32) {
    let list = GetDlgItem(GetParent(rail), 120);
    let (view, total, thumb, _) = scroll_geometry(list);
    if total <= view {
        return;
    }
    let goal = (y - thumb / 2).clamp(0, view - thumb) * (total - view) / (view - thumb).max(1);
    let mut offset = 0;
    let mut index = 0;
    let count = SendMessageW(list, LB_GETCOUNT, 0, 0).max(0) as usize;
    while index + 1 < count && offset < goal {
        offset += SendMessageW(list, LB_GETITEMHEIGHT, index, 0).max(1) as i32;
        index += 1;
    }
    SendMessageW(list, LB_SETTOPINDEX, index, 0);
    InvalidateRect(rail, null(), 1);
}

// Native LISTBOX painting and the insertion mark share one off-screen frame.
// Drawing the mark with GetDC after EndPaint exposes two different frames.
unsafe fn paint_list(hwnd: HWND, dc: HDC) {
    let mut rect: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    SetDCBrushColor(dc, PAPER);
    FillRect(dc, &rect, GetStockObject(DC_BRUSH) as HBRUSH);
    DefSubclassProc(hwnd, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
    let visual = VIEW.with(|view| view.borrow().clone());
    if let Some(view) = visual
        && let Some(y) = view.drag_indicator
    {
        let painter = Painter {
            dc,
            scale: view.scale,
        };
        let y = (y as f64 / view.scale) as i32;
        painter.stroke(
            &[(16, y), ((rect.right as f64 / view.scale) as i32 - 4, y)],
            ACCENT,
            2,
        );
        painter.round(13, y - 2, 4, 4, 2, ACCENT);
    }
}

unsafe fn buffered_list_paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let dc = BeginPaint(hwnd, &mut ps);
    let mut rect: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    if rect.right > 0 && rect.bottom > 0 {
        let buffer = CreateCompatibleDC(dc);
        let bitmap = CreateCompatibleBitmap(dc, rect.right, rect.bottom);
        if !buffer.is_null() && !bitmap.is_null() {
            let old = SelectObject(buffer, bitmap);
            paint_list(hwnd, buffer);
            // BeginPaint clips this single copy to the invalid region.
            BitBlt(dc, 0, 0, rect.right, rect.bottom, buffer, 0, 0, SRCCOPY);
            count_event(5);
            SelectObject(buffer, old);
        } else {
            paint_list(hwnd, dc);
        }
        if !bitmap.is_null() {
            DeleteObject(bitmap);
        }
        if !buffer.is_null() {
            DeleteDC(buffer);
        }
    }
    EndPaint(hwnd, &ps);
}

unsafe extern "system" fn control_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    if id == 120 {
        if msg == WM_PAINT {
            count_event(4);
            buffered_list_paint(hwnd);
            return 0;
        }
        if msg == WM_PRINTCLIENT {
            paint_list(hwnd, w as HDC);
            return 0;
        }
        if msg == WM_ERASEBKGND {
            // The buffer includes the background; never flash a separate erase.
            return 1;
        }
        if msg == WM_SETCURSOR
            && VIEW.with(|view| {
                view.borrow()
                    .as_ref()
                    .is_some_and(|v| v.drag_source.is_some())
            })
        {
            SetCursor(LoadCursorW(null_mut(), IDC_SIZENS));
            return 1;
        }
    }
    if id == 110 {
        if msg == WM_IME_STARTCOMPOSITION {
            SetPropW(hwnd, wide("rn.composing").as_ptr(), 1usize as _);
        }
        if msg == WM_IME_ENDCOMPOSITION {
            RemovePropW(hwnd, wide("rn.composing").as_ptr());
        }
    }
    if msg == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(control_proc), id);
        return DefSubclassProc(hwnd, msg, w, l);
    }
    let mut checkbox_origin = 0usize;
    let mut tag_origin = 0usize;
    if id == 120 {
        if !GetPropW(hwnd, wide("rn.dragging").as_ptr()).is_null() {
            match msg {
                WM_MOUSEMOVE => {
                    if SendMessageW(GetParent(hwnd), DRAG_MOVE, 0, l) != 0 {
                        SetCursor(LoadCursorW(null_mut(), IDC_SIZENS));
                    }
                    return 0;
                }
                WM_LBUTTONUP => {
                    SendMessageW(GetParent(hwnd), DRAG_DROP, 0, l);
                    return 0;
                }
                WM_CANCELMODE | WM_CAPTURECHANGED
                    if msg == WM_CANCELMODE || GetCapture() != hwnd =>
                {
                    SendMessageW(GetParent(hwnd), DRAG_CANCEL, 0, 0);
                }
                _ => {}
            }
        }
        if msg == WM_LBUTTONUP {
            checkbox_origin = GetPropW(hwnd, wide("rn.checkbox").as_ptr()) as usize;
            tag_origin = GetPropW(hwnd, wide("rn.tag").as_ptr()) as usize;
            RemovePropW(hwnd, wide("rn.checkbox").as_ptr());
            RemovePropW(hwnd, wide("rn.tag").as_ptr());
        }
    }
    if id == 121 {
        if msg == WM_PAINT || msg == WM_PRINTCLIENT {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = if msg == WM_PAINT {
                BeginPaint(hwnd, &mut ps)
            } else {
                w as HDC
            };
            let scale = visual_scale(hwnd);
            let p = Painter { dc, scale };
            let (view, total, thumb, y) = scroll_geometry(GetDlgItem(GetParent(hwnd), 120));
            p.fill(0, 0, 8, (view as f64 / scale).ceil() as i32, PAPER);
            if total > view {
                p.round(
                    2,
                    (y as f64 / scale) as i32,
                    3,
                    (thumb as f64 / scale) as i32,
                    1,
                    crate::appearance::rgb(187, 194, 188),
                );
            }
            if msg == WM_PAINT {
                EndPaint(hwnd, &ps);
            }
            return 0;
        }
        if msg == WM_LBUTTONDOWN {
            SetCapture(hwnd);
            drag_scroll(hwnd, (l >> 16) as u16 as i16 as i32);
            return 0;
        }
        if msg == WM_MOUSEMOVE && GetCapture() == hwnd {
            drag_scroll(hwnd, (l >> 16) as u16 as i16 as i32);
            return 0;
        }
        if msg == WM_LBUTTONUP {
            ReleaseCapture();
            return 0;
        }
        if msg == WM_MOUSEWHEEL {
            return SendMessageW(GetDlgItem(GetParent(hwnd), 120), msg, w, l);
        }
    }
    // A clipped final row is not drawn and must never complete an invisible task.
    if id == 120
        && [
            WM_LBUTTONDOWN,
            WM_LBUTTONUP,
            WM_LBUTTONDBLCLK,
            WM_RBUTTONDOWN,
            WM_RBUTTONUP,
        ]
        .contains(&msg)
    {
        let item = SendMessageW(hwnd, LB_ITEMFROMPOINT, 0, l);
        if item >> 16 == 0 {
            let mut row: RECT = std::mem::zeroed();
            let mut view: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut view);
            if SendMessageW(
                hwnd,
                LB_GETITEMRECT,
                (item & 0xffff) as usize,
                (&mut row as *mut RECT) as isize,
            ) != LB_ERR as isize
                && row.top > 0
                && row.bottom > view.bottom
            {
                if msg == WM_LBUTTONUP && GetCapture() == hwnd {
                    ReleaseCapture();
                }
                return 0;
            }
        }
    }
    if id == 120 && msg == WM_LBUTTONDOWN {
        let point = mouse_point(l);
        let scale = visual_scale(hwnd);
        let mut bounds: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut bounds);
        let item = SendMessageW(hwnd, LB_ITEMFROMPOINT, 0, l);
        RemovePropW(hwnd, wide("rn.checkbox").as_ptr());
        RemovePropW(hwnd, wide("rn.tag").as_ptr());
        if item >> 16 == 0 {
            let index = (item & 0xffff) as usize;
            if point.x >= (14.0 * scale) as i32 && point.x < (42.0 * scale) as i32 {
                SetPropW(hwnd, wide("rn.checkbox").as_ptr(), (index + 1) as _);
            } else if point.x >= bounds.right - (76.0 * scale) as i32 {
                SetPropW(hwnd, wide("rn.tag").as_ptr(), (index + 1) as _);
            } else {
                SendMessageW(hwnd, LB_SETCURSEL, index, 0);
                SendMessageW(
                    GetParent(hwnd),
                    WM_COMMAND,
                    120 | ((LBN_SELCHANGE as usize) << 16),
                    0,
                );
                // Focus changes send synchronous WM_COMMAND notifications. Finish them
                // before arming a drag so the first drag from the input is not canceled.
                SetFocus(hwnd);
                if SendMessageW(GetParent(hwnd), DRAG_START, index, l) != 0 {
                    SetPropW(hwnd, wide("rn.dragging").as_ptr(), 1usize as _);
                    SetCapture(hwnd);
                }
                return 0;
            }
        }
    }
    if id == 120 && msg == WM_MOUSEWHEEL {
        let top = SendMessageW(hwnd, LB_GETTOPINDEX, 0, 0);
        let accumulated =
            GetPropW(hwnd, wide("rn.wheel").as_ptr()) as isize + (w >> 16) as u16 as i16 as isize;
        let lines = accumulated / 120;
        if accumulated % 120 == 0 {
            RemovePropW(hwnd, wide("rn.wheel").as_ptr());
        } else {
            SetPropW(hwnd, wide("rn.wheel").as_ptr(), (accumulated % 120) as _);
        }
        if lines == 0 {
            return 0;
        }
        let count = SendMessageW(hwnd, LB_GETCOUNT, 0, 0).max(1);
        SendMessageW(
            hwnd,
            LB_SETTOPINDEX,
            (top - lines * 3).clamp(0, count - 1) as usize,
            0,
        );
        InvalidateRect(GetDlgItem(GetParent(hwnd), 121), null(), 1);
        return 0;
    }
    if msg == WM_MOUSEMOVE
        && ([
            111, 112, 113, 116, 117, 118, 119, 138, 142, 206, 207, 208, 209, 210, 214, 215,
        ]
        .contains(&id)
            || (300..=332).contains(&id))
        && GetPropW(hwnd, wide("rn.hover").as_ptr()).is_null()
    {
        SetPropW(hwnd, wide("rn.hover").as_ptr(), 1usize as _);
        InvalidateRect(hwnd, null(), 0);
        let mut event = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        TrackMouseEvent(&mut event);
    }
    if msg == WM_MOUSELEAVE && !GetPropW(hwnd, wide("rn.hover").as_ptr()).is_null() {
        RemovePropW(hwnd, wide("rn.hover").as_ptr());
        InvalidateRect(hwnd, null(), 0);
    }
    if id == 120 && msg == WM_LBUTTONUP {
        let scale = visual_scale(hwnd);
        let mut bounds: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut bounds);
        let item = SendMessageW(hwnd, LB_ITEMFROMPOINT, 0, l);
        if (l as u16 as i16 as i32) >= bounds.right - (76.0 * scale) as i32
            && item >> 16 == 0
            && tag_origin == (item & 0xffff) as usize + 1
        {
            DefSubclassProc(hwnd, msg, w, l);
            SendMessageW(hwnd, LB_SETCURSEL, (item & 0xffff) as usize, 0);
            SendMessageW(
                GetParent(hwnd),
                WM_COMMAND,
                120 | ((LBN_SELCHANGE as usize) << 16),
                0,
            );
            PostMessageW(GetParent(hwnd), WM_COMMAND, 930, 0);
            return 0;
        }
    }
    if id == 120 && msg == WM_LBUTTONUP {
        let item = SendMessageW(hwnd, LB_ITEMFROMPOINT, 0, l);
        let scale = visual_scale(hwnd);
        if (14.0 * scale) as i32 <= (l as u16 as i16 as i32)
            && (l as u16 as i16 as i32) < (42.0 * scale) as i32
            && item >> 16 == 0
            && checkbox_origin == (item & 0xffff) as usize + 1
        {
            // Let the native list finish selection and release mouse capture first.
            DefSubclassProc(hwnd, msg, w, l);
            SendMessageW(hwnd, LB_SETCURSEL, (item & 0xffff) as usize, 0);
            SendMessageW(
                GetParent(hwnd),
                WM_COMMAND,
                120 | ((LBN_SELCHANGE as usize) << 16),
                0,
            );
            SendMessageW(GetParent(hwnd), WM_COMMAND, 138, 0);
            return 0;
        }
    }
    if id == 120 && msg == WM_RBUTTONDOWN {
        let item = SendMessageW(hwnd, LB_ITEMFROMPOINT, 0, l);
        if (item >> 16) == 0 {
            SendMessageW(hwnd, LB_SETCURSEL, (item & 0xffff) as usize, 0);
            SendMessageW(
                GetParent(hwnd),
                WM_COMMAND,
                120 | ((LBN_SELCHANGE as usize) << 16),
                0,
            );
        }
    }
    if id == 120 && msg == WM_CONTEXTMENU {
        PostMessageW(GetParent(hwnd), WM_CONTEXTMENU, w, l);
        return 0;
    }
    let result = DefSubclassProc(hwnd, msg, w, l);
    if id == 120 && [WM_KEYDOWN, WM_VSCROLL, WM_LBUTTONUP].contains(&msg) {
        InvalidateRect(GetDlgItem(GetParent(hwnd), 121), null(), 1);
    }
    if msg == WM_PAINT && [110, 115, 134, 203].contains(&id) && GetWindowTextLengthW(hwnd) == 0 {
        let dc = GetDC(hwnd);
        let scale = visual_scale(hwnd);
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut r);
        let p = Painter { dc, scale };
        let font = SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT;
        p.text(
            match id {
                110 => "記下一件事，Enter 新增",
                115 => "搜尋便條",
                134 => "例如：先看測試結果",
                _ => "留空沿用已存金鑰",
            },
            font,
            MUTED,
            0,
            0,
            (r.right as f64 / scale) as i32,
            (r.bottom as f64 / scale) as i32,
            DT_SINGLELINE,
        );
        ReleaseDC(hwnd, dc);
    }
    result
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_APP + 105 && std::env::var_os("RETURN_NOTE_DATA").is_some() {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<App>;
        if !ptr.is_null() {
            let app = (*ptr).borrow();
            SetFocus(app.h(112));
            return (GetFocus() == app.h(112)) as isize;
        }
        return 0;
    }
    if msg == WM_APP + 103 && std::env::var_os("RETURN_NOTE_DATA").is_some() {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<App>;
        if !ptr.is_null() {
            let app = (*ptr).borrow_mut();
            RedrawWindow(
                app.h(120),
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_UPDATENOW,
            );
        }
        return 0;
    }
    if (WM_APP + 100..=WM_APP + 102).contains(&msg)
        && std::env::var_os("RETURN_NOTE_DATA").is_some()
    {
        return match msg {
            value if value == WM_APP + 100 => {
                PAINT_STATS.with(|stats| stats.get().get(w).copied().unwrap_or(0)) as isize
            }
            value if value == WM_APP + 101 => {
                PAINT_STATS.with(|stats| stats.set([0; 6]));
                0
            }
            _ => VIEW.with(|view| view.borrow().as_ref().is_some_and(|v| v.toast.is_empty()))
                as isize,
        };
    }
    if [WM_PAINT, WM_PRINTCLIENT, WM_DRAWITEM].contains(&msg) {
        let visual = VIEW.with(|view| view.borrow().clone());
        if msg == WM_PAINT {
            count_event(0);
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            if let Some(view) = visual {
                view.paint(dc);
            }
            EndPaint(hwnd, &ps);
        } else if let Some(view) = visual {
            if msg == WM_PRINTCLIENT {
                view.paint(w as HDC);
            } else {
                count_event(1);
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<App>;
                if !ptr.is_null() && (*ptr).try_borrow().is_err() {
                    count_event(3);
                }
                view.draw(&*(l as *const DRAWITEMSTRUCT));
            }
        }
        return if msg == WM_DRAWITEM { 1 } else { 0 };
    }
    if msg == WM_CTLCOLOREDIT {
        let id = GetDlgCtrlID(l as HWND);
        let color = if id == 131 { PAPER } else { INPUT };
        SetBkColor(w as HDC, color);
        SetTextColor(w as HDC, INK);
        SetDCBrushColor(w as HDC, color);
        return GetStockObject(DC_BRUSH) as isize;
    }
    if msg == WM_CTLCOLORSTATIC || msg == WM_CTLCOLORLISTBOX {
        SetBkColor(w as HDC, PAPER);
        SetTextColor(w as HDC, INK);
        SetDCBrushColor(w as HDC, PAPER);
        return GetStockObject(DC_BRUSH) as isize;
    }
    if msg == WM_ERASEBKGND {
        // The composited paint pass fills the background and children together.
        return 1;
    }
    if msg == WM_DESTROY {
        UnregisterHotKey(hwnd, 1);
        PostQuitMessage(0);
        return 0;
    }
    if msg == WM_MEASUREITEM {
        let d = &mut *(l as *mut MEASUREITEMSTRUCT);
        d.itemHeight = (36.0 * visual_scale(hwnd)).round() as u32;
        return 1;
    }
    if msg == WM_NCCALCSIZE && w != 0 {
        return 0;
    }
    if msg == WM_NCHITTEST {
        let mut point = POINT {
            x: (l as u16 as i16) as i32,
            y: ((l >> 16) as u16 as i16) as i32,
        };
        ScreenToClient(hwnd, &mut point);
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut r);
        let edge = 6;
        let left = point.x < edge;
        let right = point.x >= r.right - edge;
        let top = point.y < edge;
        let bottom = point.y >= r.bottom - edge;
        return (if top && left {
            HTTOPLEFT
        } else if top && right {
            HTTOPRIGHT
        } else if bottom && left {
            HTBOTTOMLEFT
        } else if bottom && right {
            HTBOTTOMRIGHT
        } else if left {
            HTLEFT
        } else if right {
            HTRIGHT
        } else if top {
            HTTOP
        } else if bottom {
            HTBOTTOM
        } else if point.y < (48.0 * visual_scale(hwnd)) as i32 {
            HTCAPTION
        } else {
            HTCLIENT
        }) as isize;
    }
    if msg == WM_NCLBUTTONDBLCLK {
        return 0;
    }
    if msg == WM_GETMINMAXINFO {
        let m = &mut *(l as *mut MINMAXINFO);
        let scale = visual_scale(hwnd);
        m.ptMinTrackSize.x = (320.0 * scale) as i32;
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<App>;
        let min_height = if !ptr.is_null() {
            (*ptr)
                .try_borrow()
                .map(|a| {
                    if a.settings {
                        550.0
                    } else if a.editing {
                        440.0
                    } else {
                        280.0
                    }
                })
                .unwrap_or(280.0)
        } else {
            280.0
        };
        m.ptMinTrackSize.y = (min_height * scale) as i32;
        return 0;
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<App>;
    if !ptr.is_null()
        && let Ok(mut app) = (*ptr).try_borrow_mut()
    {
        match msg {
            DRAG_START => {
                return match app.begin_drag(w, mouse_point(l)) {
                    Ok(started) => started as isize,
                    Err(error) => {
                        app.notice(&error);
                        0
                    }
                };
            }
            DRAG_MOVE => {
                return app.update_drag(mouse_point(l), false) as isize;
            }
            DRAG_DROP => {
                if let Err(error) = app.finish_drag(mouse_point(l)) {
                    app.notice(&error);
                }
                return 0;
            }
            DRAG_CANCEL => {
                app.cancel_drag();
                return 0;
            }
            WM_ACTIVATE if (w & 0xffff) as u32 == WA_INACTIVE => {
                app.cancel_drag();
            }
            WM_CONTEXTMENU => {
                if let Err(e) = app.menu(true) {
                    app.notice(&e);
                }
                return 0;
            }
            WM_EXITSIZEMOVE => {
                app.remember_main_size();
                return 0;
            }
            WM_SIZE => {
                app.cancel_drag();
                app.layout();
                return 0;
            }
            WM_DPICHANGED => {
                app.cancel_drag();
                app.scale = (w & 0xffff) as f64 / 96.0
                    * f64::from(app.board.settings.zoom_percent.clamp(70, 125))
                    / 100.0;
                app.update_fonts();
                let r = &*(l as *const RECT);
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                app.layout();
                return 0;
            }
            WM_COMMAND => {
                if let Err(e) = app.action((w & 0xffff) as i32, ((w >> 16) & 0xffff) as u32) {
                    app.notice(&e);
                }
                return 0;
            }
            WM_TIMER => {
                if w == 5 {
                    if !app.settings && !app.editing && app.drag.is_none() && IsIconic(hwnd) == 0 {
                        app.publish_view();
                        for (i, id) in app.rows.iter().enumerate() {
                            if app
                                .board
                                .notes
                                .iter()
                                .any(|n| n.id == *id && n.timer_started.is_some())
                            {
                                let mut r: RECT = Default::default();
                                SendMessageW(
                                    app.h(120),
                                    LB_GETITEMRECT,
                                    i,
                                    (&mut r as *mut RECT) as isize,
                                );
                                InvalidateRect(app.h(120), &r, 0);
                            }
                        }
                    }
                    return 0;
                }
                if w == 4 {
                    if let Some(drag) = &app.drag {
                        let point = drag.point;
                        app.update_drag(point, true);
                    }
                    return 0;
                }
                if w == 3 {
                    KillTimer(hwnd, 3);
                    if !app.toast_error {
                        app.toast.clear();
                        app.repaint_notice();
                    }
                    return 0;
                }
                if w == 2 {
                    KillTimer(hwnd, 2);
                } else {
                    app.reminders();
                }
                if let Err(e) = app.results() {
                    app.notice(&e);
                }
                return 0;
            }
            WM_CLOSE => {
                if let Err(e) = app.save_edits() {
                    app.notice(&e);
                } else {
                    ShowWindow(hwnd, SW_MINIMIZE);
                }
                return 0;
            }
            WM_HOTKEY => {
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
                if app.settings {
                    let _ = app.settings(false);
                } else if app.editing {
                    let _ = app.compact(true);
                }
                SetFocus(app.h(110));
                return 0;
            }
            RESULT => {
                if let Err(e) = app.results() {
                    app.notice(&e);
                }
                return 0;
            }
            _ => {}
        }
    }
    DefWindowProcW(hwnd, msg, w, l)
}

pub fn run() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let dir = std::env::var_os("RETURN_NOTE_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default())
                    .join("ReturnNote")
            });
        // Separate test data gets its own instance, with no access to the production key.
        let isolated = std::env::var_os("RETURN_NOTE_DATA").is_some();
        let mutex = CreateMutexW(
            null(),
            0,
            wide(if isolated {
                "Local\\ReturnNote.Test.v1"
            } else {
                "Local\\ReturnNote.v1"
            })
            .as_ptr(),
        );
        if mutex.is_null() {
            message(null_mut(), "無法建立程式鎖，請重新啟動。");
            return;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let h = FindWindowW(
                wide(CLASS).as_ptr(),
                wide(if isolated {
                    "回來 · 測試"
                } else {
                    "回來"
                })
                .as_ptr(),
            );
            if !h.is_null() {
                PostMessageW(h, WM_HOTKEY, 1, 0);
            }
            CloseHandle(mutex);
            return;
        }
        let storage = match Storage::new(&dir) {
            Ok(s) => s,
            Err(e) => {
                message(null_mut(), &e);
                CloseHandle(mutex);
                return;
            }
        };
        let board = match storage.load() {
            Ok(b) => b,
            Err(e) => {
                message(null_mut(), &format!("{e}\r\n{}", storage.path.display()));
                CloseHandle(mutex);
                return;
            }
        };
        let _drawing = crate::appearance::start();
        let instance = GetModuleHandleW(null());
        let cls = wide(CLASS);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: cls.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hIcon: LoadIconW(instance, 1usize as _),
            style: CS_HREDRAW | CS_VREDRAW,
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let scale = GetDpiForSystem() as f64 / 96.0
            * f64::from(board.settings.zoom_percent.clamp(70, 125))
            / 100.0;
        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW | WS_EX_COMPOSITED,
            cls.as_ptr(),
            wide(if isolated {
                "回來 · 測試"
            } else {
                "回來"
            })
            .as_ptr(),
            WS_POPUP | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            (392.0 * scale) as i32,
            (420.0 * scale) as i32,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            message(null_mut(), "視窗開啟失敗。");
            CloseHandle(mutex);
            return;
        }
        let corner = 2u32;
        DwmSetWindowAttribute(hwnd, 33, (&corner as *const u32).cast(), 4);
        DwmSetWindowAttribute(hwnd, 34, (&LINE as *const u32).cast(), 4);
        let margins = MARGINS {
            cxLeftWidth: 1,
            cxRightWidth: 1,
            cyTopHeight: 1,
            cyBottomHeight: 1,
        };
        DwmExtendFrameIntoClientArea(hwnd, &margins);
        let make = |size, weight| make_font(size, weight, scale);
        let (font, bold, big, tiny) = (
            make(15.0, 400),
            make(14.0, 600),
            make(21.0, 500),
            make(12.0, 400),
        );
        let (tx, rx) = mpsc::channel();
        let app = Box::new(RefCell::new(App {
            hwnd,
            controls: HashMap::new(),
            font,
            bold,
            big,
            tiny,
            editing: false,
            searching: false,
            main_size: (392, 420),
            drag: None,
            toast: String::new(),
            toast_error: false,
            tips: Vec::new(),
            board,
            storage,
            selected: None,
            rows: vec![],
            managing: false,
            settings: false,
            dirty: false,
            filter: 0,
            category_filter: None,
            tx,
            rx,
            queue: VecDeque::new(),
            busy: false,
            in_flight: None,
            ai_paused: false,
            ai_status: String::new(),
            hotkey: false,
            scale,
        }));
        let ptr = Box::into_raw(app);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        (*ptr).borrow_mut().init();
        // Window creation enforces a minimum before the saved zoom is available.
        (*ptr).borrow().resize();
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if msg.message == WM_KEYDOWN || msg.message == WM_SYSKEYDOWN {
                if GetFocus() == (*ptr).borrow().h(120)
                    && GetKeyState(VK_MENU as i32) < 0
                    && [VK_UP as usize, VK_DOWN as usize].contains(&msg.wParam)
                {
                    SendMessageW(
                        hwnd,
                        WM_COMMAND,
                        if msg.wParam == VK_UP as usize {
                            940
                        } else {
                            941
                        },
                        0,
                    );
                    continue;
                }
                if msg.wParam == VK_ESCAPE as usize && (*ptr).borrow().drag.is_some() {
                    SendMessageW(hwnd, DRAG_CANCEL, 0, 0);
                    continue;
                }
                let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
                if msg.wParam == VK_RETURN as usize {
                    let app = (*ptr).borrow();
                    let capture = GetFocus() == app.h(110)
                        && !app.settings
                        && !app.editing
                        && (ctrl || GetKeyState(VK_SHIFT as i32) >= 0)
                        && GetPropW(app.h(110), wide("rn.composing").as_ptr()).is_null();
                    drop(app);
                    if capture {
                        SendMessageW(hwnd, WM_COMMAND, 111, 0);
                        continue;
                    }
                }
                if msg.wParam == VK_ESCAPE as usize {
                    let app = (*ptr).borrow();
                    let action = if app.settings {
                        210
                    } else if app.editing {
                        116
                    } else if app.searching {
                        904
                    } else {
                        117
                    };
                    drop(app);
                    SendMessageW(hwnd, WM_COMMAND, action, 0);
                    continue;
                }
                if msg.wParam == VK_SPACE as usize && GetFocus() == (*ptr).borrow().h(120) {
                    SendMessageW(hwnd, WM_COMMAND, 138, 0);
                    continue;
                }
                if ctrl && msg.wParam == 'F' as usize {
                    SendMessageW(hwnd, WM_COMMAND, 904, 0);
                    continue;
                }
            }
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        VIEW.with(|view| *view.borrow_mut() = None);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        let app = Box::from_raw(ptr).into_inner();
        for font in [app.font, app.bold, app.big, app.tiny] {
            DeleteObject(font);
        }
        drop(app);
        CloseHandle(mutex);
    }
}
