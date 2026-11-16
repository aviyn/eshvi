//! Decoding of msgpack `redraw` notifications into typed events.

use grid::{
    ChunkStyle, CursorShape, FloatAnchor, HlAttr, LineCell, ModeInfo, OptionValue, PmItem, RedrawEvent, StyledChunk,
    Underline,
};
use nvim_rs::Value;

fn u64_of(v: &Value) -> u64 {
    v.as_u64().or_else(|| v.as_i64().map(|i| i.max(0) as u64)).unwrap_or(0)
}

fn i64_of(v: &Value) -> i64 {
    v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)).unwrap_or(0)
}

fn usize_of(v: &Value) -> usize {
    u64_of(v) as usize
}

fn f64_of(v: &Value) -> f64 {
    v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)).unwrap_or(0.0)
}

fn str_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.as_str().map(str::to_owned).unwrap_or_else(|| String::from_utf8_lossy(s.as_bytes()).into_owned()),
        Value::Binary(b) => String::from_utf8_lossy(b).into_owned(),
        _ => String::new(),
    }
}

fn bool_of(v: &Value) -> bool {
    v.as_bool().unwrap_or(false)
}

/// Window/Buffer/Tabpage handles arrive as msgpack ext types wrapping an integer.
fn handle_of(v: &Value) -> i64 {
    match v {
        Value::Ext(_, data) => rmpv_decode_int(data),
        other => i64_of(other),
    }
}

fn rmpv_decode_int(data: &[u8]) -> i64 {
    // Ext payloads are themselves msgpack-encoded integers.
    let Some(&b) = data.first() else { return 0 };
    let be = |n: usize| -> u64 {
        data.get(1..1 + n).map(|s| s.iter().fold(0u64, |acc, &x| (acc << 8) | x as u64)).unwrap_or(0)
    };
    match b {
        0x00..=0x7f => b as i64,
        0xe0..=0xff => (b as i8) as i64,
        0xcc => be(1) as i64,
        0xcd => be(2) as i64,
        0xce => be(4) as i64,
        0xcf => be(8) as i64,
        0xd0 => be(1) as u8 as i8 as i64,
        0xd1 => be(2) as u16 as i16 as i64,
        0xd2 => be(4) as u32 as i32 as i64,
        0xd3 => be(8) as i64,
        _ => 0,
    }
}

fn map_get<'a>(map: &'a [(Value, Value)], key: &str) -> Option<&'a Value> {
    map.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v)
}

fn as_map(v: &Value) -> &[(Value, Value)] {
    v.as_map().map(Vec::as_slice).unwrap_or(&[])
}

fn as_arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

pub fn parse_hl_attr(v: &Value) -> HlAttr {
    let m = as_map(v);
    let color = |k| map_get(m, k).map(|v| u64_of(v) as u32);
    let flag = |k| map_get(m, k).map(bool_of).unwrap_or(false);
    let underline = if flag("undercurl") {
        Underline::Curl
    } else if flag("underdouble") {
        Underline::Double
    } else if flag("underdotted") {
        Underline::Dotted
    } else if flag("underdashed") {
        Underline::Dashed
    } else if flag("underline") {
        Underline::Line
    } else {
        Underline::None
    };
    HlAttr {
        foreground: color("foreground"),
        background: color("background"),
        special: color("special"),
        reverse: flag("reverse"),
        italic: flag("italic"),
        bold: flag("bold"),
        strikethrough: flag("strikethrough"),
        underline,
        blend: map_get(m, "blend").map(|v| u64_of(v).min(100) as u8).unwrap_or(0),
    }
}

fn parse_mode_info(v: &Value) -> ModeInfo {
    let m = as_map(v);
    let s = |k| map_get(m, k).map(str_of).unwrap_or_default();
    let n = |k| map_get(m, k).map(u64_of).unwrap_or(0);
    ModeInfo {
        name: s("name"),
        short_name: s("short_name"),
        cursor_shape: match s("cursor_shape").as_str() {
            "horizontal" => CursorShape::Horizontal,
            "vertical" => CursorShape::Vertical,
            _ => CursorShape::Block,
        },
        cell_percentage: map_get(m, "cell_percentage").map(|v| u64_of(v) as u32),
        attr_id: n("attr_id"),
        blinkwait: n("blinkwait"),
        blinkon: n("blinkon"),
        blinkoff: n("blinkoff"),
    }
}

fn parse_grid_line(a: &[Value]) -> Option<RedrawEvent> {
    let [grid, row, col_start, cells, ..] = a else { return None };
    let mut hl_id = 0;
    let cells = as_arr(cells)
        .iter()
        .map(|c| {
            let c = as_arr(c);
            if let Some(h) = c.get(1) {
                hl_id = u64_of(h);
            }
            LineCell {
                text: c.first().map(str_of).unwrap_or_default(),
                hl_id,
                repeat: c.get(2).map(usize_of).unwrap_or(1),
            }
        })
        .collect();
    Some(RedrawEvent::GridLine { grid: u64_of(grid), row: usize_of(row), col_start: usize_of(col_start), cells })
}

/// Group name of one `ext_hlstate` info entry (`hi_name` or `ui_name`).
fn hl_info_name(v: &Value) -> Option<String> {
    let m = as_map(v);
    map_get(m, "hi_name").or_else(|| map_get(m, "ui_name")).map(str_of).filter(|s| !s.is_empty())
}

/// `[[attrs, text, hl_id?], ...]` where attrs is an attr id or a dict.
fn parse_chunks(v: &Value) -> Vec<StyledChunk> {
    as_arr(v)
        .iter()
        .map(|c| {
            let c = as_arr(c);
            let style = match c.first() {
                Some(Value::Map(_)) => ChunkStyle::Attr(parse_hl_attr(&c[0])),
                Some(id) => ChunkStyle::Id(u64_of(id)),
                None => ChunkStyle::Id(0),
            };
            StyledChunk { style, text: c.get(1).map(str_of).unwrap_or_default() }
        })
        .collect()
}

fn parse_option_value(v: &Value) -> OptionValue {
    match v {
        Value::Boolean(b) => OptionValue::Bool(*b),
        Value::Integer(_) => OptionValue::Int(i64_of(v)),
        Value::String(_) => OptionValue::Str(str_of(v)),
        _ => OptionValue::Other,
    }
}

/// Parse a single event invocation (one argument tuple of a redraw event).
pub fn parse_event(name: &str, a: &[Value]) -> Option<RedrawEvent> {
    use RedrawEvent::*;
    let g = |i: usize| a.get(i).map(u64_of).unwrap_or(0);
    let us = |i: usize| a.get(i).map(usize_of).unwrap_or(0);
    let i = |i: usize| a.get(i).map(i64_of).unwrap_or(0);
    let s = |i: usize| a.get(i).map(str_of).unwrap_or_default();
    let b = |i: usize| a.get(i).map(bool_of).unwrap_or(false);
    Some(match name {
        "set_title" => SetTitle(s(0)),
        "mode_info_set" => ModeInfoSet {
            cursor_style_enabled: b(0),
            modes: a.get(1).map(as_arr).unwrap_or(&[]).iter().map(parse_mode_info).collect(),
        },
        "mode_change" => ModeChange { mode: s(0), mode_idx: us(1) },
        "option_set" => OptionSet { name: s(0), value: a.get(1).map(parse_option_value).unwrap_or(OptionValue::Other) },
        "mouse_on" => MouseOn,
        "mouse_off" => MouseOff,
        "busy_start" => BusyStart,
        "busy_stop" => BusyStop,
        "flush" => Flush,
        "ui_send" => UiSend(match a.first() {
            Some(Value::String(s)) => s.as_bytes().to_vec(),
            Some(Value::Binary(b)) => b.clone(),
            _ => Vec::new(),
        }),
        "grid_resize" => GridResize { grid: g(0), width: us(1), height: us(2) },
        "default_colors_set" => DefaultColorsSet { fg: g(0) as u32, bg: g(1) as u32, sp: g(2) as u32 },
        "hl_attr_define" => HlAttrDefine {
            id: g(0),
            attr: a.get(1).map(parse_hl_attr).unwrap_or_default(),
            groups: a.get(3).map(as_arr).unwrap_or(&[]).iter().filter_map(hl_info_name).collect(),
        },
        "hl_group_set" => HlGroupSet { name: s(0), id: g(1) },
        "grid_line" => return parse_grid_line(a),
        "grid_clear" => GridClear { grid: g(0) },
        "grid_destroy" => GridDestroy { grid: g(0) },
        "grid_cursor_goto" => GridCursorGoto { grid: g(0), row: us(1), col: us(2) },
        "grid_scroll" => GridScroll {
            grid: g(0),
            top: us(1),
            bot: us(2),
            left: us(3),
            right: us(4),
            rows: i(5),
            cols: i(6),
        },
        "win_pos" => WinPos {
            grid: g(0),
            win: a.get(1).map(handle_of).unwrap_or(0),
            start_row: us(2),
            start_col: us(3),
            width: us(4),
            height: us(5),
        },
        "win_float_pos" => WinFloatPos {
            grid: g(0),
            win: a.get(1).map(handle_of).unwrap_or(0),
            anchor: match s(2).as_str() {
                "NE" => FloatAnchor::NE,
                "SW" => FloatAnchor::SW,
                "SE" => FloatAnchor::SE,
                _ => FloatAnchor::NW,
            },
            anchor_grid: g(3),
            anchor_row: a.get(4).map(f64_of).unwrap_or(0.0),
            anchor_col: a.get(5).map(f64_of).unwrap_or(0.0),
            mouse_enabled: b(6),
            zindex: i(7),
            compindex: i(8),
            screen_row: i(9),
            screen_col: i(10),
        },
        "win_external_pos" => WinExternalPos { grid: g(0), win: a.get(1).map(handle_of).unwrap_or(0) },
        "win_hide" => WinHide { grid: g(0) },
        "win_close" => WinClose { grid: g(0) },
        "msg_set_pos" => MsgSetPos {
            grid: g(0),
            row: us(1),
            scrolled: b(2),
            sep_char: s(3),
            zindex: i(4),
            compindex: i(5),
        },
        "win_viewport" => WinViewport {
            grid: g(0),
            win: a.get(1).map(handle_of).unwrap_or(0),
            topline: i(2),
            botline: i(3),
            curline: i(4),
            curcol: i(5),
            line_count: i(6),
            scroll_delta: i(7),
        },
        "win_viewport_margins" => WinViewportMargins {
            grid: g(0),
            win: a.get(1).map(handle_of).unwrap_or(0),
            top: us(2),
            bottom: us(3),
            left: us(4),
            right: us(5),
        },
        "popupmenu_show" => PopupmenuShow {
            items: a
                .first()
                .map(as_arr)
                .unwrap_or(&[])
                .iter()
                .map(|it| {
                    let it = as_arr(it);
                    let f = |i: usize| it.get(i).map(str_of).unwrap_or_default();
                    PmItem { word: f(0), kind: f(1), menu: f(2), info: f(3) }
                })
                .collect(),
            selected: i(1),
            row: i(2),
            col: i(3),
            grid: i(4),
        },
        "popupmenu_select" => PopupmenuSelect { selected: i(0) },
        "popupmenu_hide" => PopupmenuHide,
        "tabline_update" => TablineUpdate {
            current: a.first().map(handle_of).unwrap_or(0),
            tabs: a
                .get(1)
                .map(as_arr)
                .unwrap_or(&[])
                .iter()
                .map(|t| {
                    let m = as_map(t);
                    (map_get(m, "tab").map(handle_of).unwrap_or(0), map_get(m, "name").map(str_of).unwrap_or_default())
                })
                .collect(),
        },
        "cmdline_show" => CmdlineShow {
            content: a.first().map(parse_chunks).unwrap_or_default(),
            pos: us(1),
            firstc: s(2),
            prompt: s(3),
            indent: us(4),
            level: g(5),
        },
        "cmdline_pos" => CmdlinePos { pos: us(0), level: g(1) },
        "cmdline_special_char" => CmdlineSpecialChar { c: s(0), shift: b(1), level: g(2) },
        "cmdline_hide" => CmdlineHide { level: g(0) },
        "cmdline_block_show" => {
            CmdlineBlockShow { lines: a.first().map(as_arr).unwrap_or(&[]).iter().map(parse_chunks).collect() }
        }
        "cmdline_block_append" => CmdlineBlockAppend { line: a.first().map(parse_chunks).unwrap_or_default() },
        "cmdline_block_hide" => CmdlineBlockHide,
        "msg_show" => MsgShow {
            kind: s(0),
            content: a.get(1).map(parse_chunks).unwrap_or_default(),
            replace_last: b(2),
            history: b(3),
            append: b(4),
            id: match a.get(5) {
                None | Some(Value::Nil) => None,
                Some(Value::String(_)) => Some(s(5)),
                Some(v) => Some(i64_of(v).to_string()),
            },
        },
        "msg_clear" => MsgClear,
        "msg_showmode" => MsgShowmode(a.first().map(parse_chunks).unwrap_or_default()),
        "msg_showcmd" => MsgShowcmd(a.first().map(parse_chunks).unwrap_or_default()),
        "msg_ruler" => MsgRuler(a.first().map(parse_chunks).unwrap_or_default()),
        "msg_history_show" => MsgHistoryShow(
            a.first()
                .map(as_arr)
                .unwrap_or(&[])
                .iter()
                .map(|e| {
                    let e = as_arr(e);
                    (
                        e.first().map(str_of).unwrap_or_default(),
                        e.get(1).map(parse_chunks).unwrap_or_default(),
                        e.get(2).map(bool_of).unwrap_or(false),
                    )
                })
                .collect(),
        ),
        _ => return None,
    })
}

/// Parse the arguments of a `redraw` notification: a list of
/// `[name, args...]` arrays, each with one or more argument tuples.
pub fn parse_redraw(args: &[Value], out: &mut Vec<RedrawEvent>) {
    for ev in args {
        let ev = as_arr(ev);
        let Some(name) = ev.first().and_then(Value::as_str) else { continue };
        for call in &ev[1..] {
            if let Some(e) = parse_event(name, as_arr(call)) {
                out.push(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v<T: Into<Value>>(x: T) -> Value {
        x.into()
    }

    fn arr(items: Vec<Value>) -> Value {
        Value::Array(items)
    }

    #[test]
    fn grid_line_carries_hl_and_repeat() {
        let cells = arr(vec![
            arr(vec![v("a"), v(5)]),
            arr(vec![v("b")]),
            arr(vec![v(" "), v(7), v(3)]),
        ]);
        let ev = arr(vec![v("grid_line"), arr(vec![v(2), v(1), v(4), cells, v(false)])]);
        let mut out = vec![];
        parse_redraw(&[ev], &mut out);
        assert_eq!(
            out,
            vec![RedrawEvent::GridLine {
                grid: 2,
                row: 1,
                col_start: 4,
                cells: vec![
                    LineCell { text: "a".into(), hl_id: 5, repeat: 1 },
                    LineCell { text: "b".into(), hl_id: 5, repeat: 1 },
                    LineCell { text: " ".into(), hl_id: 7, repeat: 3 },
                ],
            }]
        );
    }

    #[test]
    fn multiple_calls_per_event_and_flush() {
        let ev1 = arr(vec![
            v("grid_scroll"),
            arr(vec![v(1), v(0), v(10), v(0), v(80), v(-2), v(0)]),
            arr(vec![v(1), v(0), v(10), v(0), v(80), v(1), v(0)]),
        ]);
        let ev2 = arr(vec![v("flush"), arr(vec![])]);
        let mut out = vec![];
        parse_redraw(&[ev1, ev2], &mut out);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0], RedrawEvent::GridScroll { rows: -2, .. }));
        assert_eq!(out[2], RedrawEvent::Flush);
    }

    #[test]
    fn hl_attr_define_parses_flags() {
        let attr = Value::Map(vec![
            (v("foreground"), v(0x112233)),
            (v("bold"), v(true)),
            (v("undercurl"), v(true)),
        ]);
        let info = arr(vec![Value::Map(vec![(v("kind"), v("syntax")), (v("hi_name"), v("NvimTreeFolderIcon")), (v("id"), v(80))])]);
        let ev = arr(vec![v("hl_attr_define"), arr(vec![v(3), attr, Value::Map(vec![]), info])]);
        let mut out = vec![];
        parse_redraw(&[ev], &mut out);
        let RedrawEvent::HlAttrDefine { id, attr, groups } = &out[0] else { panic!() };
        assert_eq!(groups, &["NvimTreeFolderIcon"]);
        assert_eq!(*id, 3);
        assert_eq!(attr.foreground, Some(0x112233));
        assert!(attr.bold);
        assert_eq!(attr.underline, Underline::Curl);
    }

    #[test]
    fn window_handle_ext_is_decoded() {
        assert_eq!(handle_of(&Value::Ext(1, vec![0xcd, 0x03, 0xe8])), 1000);
        assert_eq!(handle_of(&Value::Ext(1, vec![0x05])), 5);
    }
}
