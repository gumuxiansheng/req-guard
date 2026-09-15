//! 极简 JSON 解析（零依赖，仅够 AI 工具 PreToolUse hook 用）。
//!
//! ## 为什么不用正则抠字段
//! AI 工具传给 hook 的 payload 是 **JSON 字符串**，允许任意转义：
//! `"file_path": ".gates\u002frequirements\u002fREQ-001.comments.md"` 与
//! `".gates/requirements/REQ-001.comments.md"` 完全等价。而原先用
//! `sed -n 's/.*"file_path"...'` 抠字段，遇到这类转义（或字符串里出现转义引号、
//! 多层嵌套）就**匹配不到**，保护被静默绕过——失效方向恰好是最坏的：
//! "看着在拦，其实没拦"。
//!
//! 拦截脚本是本项目**唯一判定真相**，解析脆弱性会同时污染 L1 与 L2，
//! 故把 payload 解析下沉到 Rust 侧真解析（项目零外部依赖，不能引 serde）。
//!
//! 只实现需要的能力：解析成 [`Value`] 树 + 按键取值。数字按 f64 存，
//! 我们不消费它，只要求"能正确跳过"。

/// 解析后的 JSON 值。
#[derive(Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    /// 取对象字段（`&self` 语义：同名重复键取第一个，与大多数解析器一致）。
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// 深度优先找第一个同名键的**字符串**值。
    ///
    /// 各 AI 工具的 payload 结构并不一致（Claude 在 `tool_input` 下，其余工具
    /// 可能更扁平或更深），先按 "正常形态" 取，取不到再深搜兜底——
    /// 宁可多认，不可漏判（漏判 = 保护失效）。
    pub fn find_str(&self, key: &str) -> Option<&str> {
        match self {
            Value::Obj(items) => {
                if let Some(s) = items
                    .iter()
                    .find(|(k, _)| k == key)
                    .and_then(|(_, v)| v.as_str())
                {
                    return Some(s);
                }
                items.iter().find_map(|(_, v)| v.find_str(key))
            }
            Value::Arr(items) => items.iter().find_map(|v| v.find_str(key)),
            _ => None,
        }
    }
}

/// 解析 JSON 文本；失败返回 `None`（调用方据此走"放行给后续门禁"而非崩掉）。
pub fn parse(input: &str) -> Option<Value> {
    let mut p = Parser {
        b: input.trim().as_bytes(),
        i: 0,
    };
    let v = p.value()?;
    p.ws();
    if p.i != p.b.len() {
        return None; // 尾随垃圾：宁可判失败，也不猜
    }
    Some(v)
}

/// 取字段的通用路径：优先 `tool_input.<key>`，取不到再深搜兜底。
fn field_str(payload: &str, key: &str) -> Option<String> {
    let v = parse(payload)?;
    v.get("tool_input")
        .and_then(|ti| ti.get(key))
        .and_then(Value::as_str)
        .or_else(|| v.find_str(key))
        .map(|s| s.to_string())
}

/// 从 PreToolUse payload 里取出**被写文件的路径**。
pub fn file_path_of(payload: &str) -> Option<String> {
    field_str(payload, "file_path")
}

/// 待写入的**整篇内容**（Write 的 `content`）。
pub fn content_of(payload: &str) -> Option<String> {
    field_str(payload, "content")
}

/// 片段编辑的**新片段**（Edit 的 `new_string`；无 `content` 时作为整篇内容用）。
pub fn new_string_of(payload: &str) -> Option<String> {
    field_str(payload, "new_string")
}

/// 片段编辑的**被替换原文**（Edit 的 `old_string`）。
///
/// 存在即说明这是一次**片段编辑**而非整篇覆盖——无法与磁盘基线逐行比对，
/// 对清单文件必须禁掉（见 [`crate::gate::pretool_verdict`]）。
pub fn old_string_of(payload: &str) -> Option<String> {
    field_str(payload, "old_string")
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn ws(&mut self) {
        while matches!(
            self.peek(),
            Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
        ) {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn value(&mut self) -> Option<Value> {
        self.ws();
        match self.peek()? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => Some(Value::Str(self.string()?)),
            b't' => self.lit("true").map(|()| Value::Bool(true)),
            b'f' => self.lit("false").map(|()| Value::Bool(false)),
            b'n' => self.lit("null").map(|()| Value::Null),
            _ => self.number(),
        }
    }

    fn lit(&mut self, s: &str) -> Option<()> {
        if self.b[self.i..].starts_with(s.as_bytes()) {
            self.i += s.len();
            Some(())
        } else {
            None
        }
    }

    /// 数字：我们不消费其值，只需**正确跳过**，故容错解析（失败按 0 存）。
    fn number(&mut self) -> Option<Value> {
        let start = self.i;
        while matches!(
            self.peek(),
            Some(b'0'..=b'9') | Some(b'-') | Some(b'+') | Some(b'.') | Some(b'e') | Some(b'E')
        ) {
            self.i += 1;
        }
        if start == self.i {
            return None;
        }
        let raw = std::str::from_utf8(&self.b[start..self.i]).ok()?;
        Some(Value::Num(raw.parse::<f64>().unwrap_or(0.0)))
    }

    fn object(&mut self) -> Option<Value> {
        self.i += 1; // 跳过 '{'
        let mut items: Vec<(String, Value)> = Vec::new();
        self.ws();
        if self.peek()? == b'}' {
            self.i += 1;
            return Some(Value::Obj(items));
        }
        loop {
            self.ws();
            if self.peek()? != b'"' {
                return None;
            }
            let k = self.string()?;
            self.ws();
            if self.peek()? != b':' {
                return None;
            }
            self.i += 1;
            let v = self.value()?;
            items.push((k, v));
            self.ws();
            match self.peek()? {
                b',' => self.i += 1,
                b'}' => {
                    self.i += 1;
                    return Some(Value::Obj(items));
                }
                _ => return None,
            }
        }
    }

    fn array(&mut self) -> Option<Value> {
        self.i += 1; // 跳过 '['
        let mut items = Vec::new();
        self.ws();
        if self.peek()? == b']' {
            self.i += 1;
            return Some(Value::Arr(items));
        }
        loop {
            items.push(self.value()?);
            self.ws();
            match self.peek()? {
                b',' => self.i += 1,
                b']' => {
                    self.i += 1;
                    return Some(Value::Arr(items));
                }
                _ => return None,
            }
        }
    }

    /// 解析字符串字面量（含全部 JSON 转义，Unicode 转义会解出真实字符）。
    fn string(&mut self) -> Option<String> {
        self.i += 1; // 跳过开头 '"'
        let mut out = String::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return Some(out),
                b'\\' => {
                    let esc = *self.b.get(self.i)?;
                    self.i += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape()?),
                        _ => return None, // 非法转义：不猜，直接失败
                    }
                }
                // 未转义的控制字符不合法（JSON 规范）；多字节 UTF-8 整段拷入。
                c if c < 0x20 => return None,
                _ => {
                    let start = self.i - 1;
                    while matches!(self.peek(), Some(x) if x >= 0x80) {
                        self.i += 1;
                    }
                    out.push_str(std::str::from_utf8(&self.b[start..self.i]).ok()?);
                }
            }
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..4 {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            let d = match c {
                b'0'..=b'9' => u32::from(c - b'0'),
                b'a'..=b'f' => u32::from(c - b'a') + 10,
                b'A'..=b'F' => u32::from(c - b'A') + 10,
                _ => return None,
            };
            v = v * 16 + d;
        }
        Some(v)
    }

    /// `\uXXXX`，含代理对（如 emoji）：高位后紧跟低位才合并，否则按单字符处理。
    fn unicode_escape(&mut self) -> Option<char> {
        let mut cp = self.hex4()?;
        if (0xD800..0xDC00).contains(&cp) && self.peek() == Some(b'\\') {
            let save = self.i;
            self.i += 1;
            if self.peek() == Some(b'u') {
                self.i += 1;
                match self.hex4() {
                    Some(lo) if (0xDC00..0xE000).contains(&lo) => {
                        cp = 0x1_0000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                    }
                    _ => self.i = save,
                }
            } else {
                self.i = save;
            }
        }
        char::from_u32(cp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析基础字面量与嵌套结构() {
        let v = parse(r#"{"a":1,"b":[true,false,null],"c":{"d":"x"}}"#).unwrap();
        assert_eq!(v.get("a").unwrap().as_str(), None);
        assert_eq!(v.get("c").unwrap().get("d").unwrap().as_str(), Some("x"));
        assert!(matches!(v.get("b"), Some(Value::Arr(x)) if x.len() == 3));
    }

    #[test]
    fn 转义全部解开() {
        let v = parse(r#"{"s":"a\"b\\c\/d\be\ff\ng\rh\ti"}"#).unwrap();
        assert_eq!(
            v.get("s").unwrap().as_str(),
            Some("a\"b\\c/d\u{8}e\u{c}f\ng\rh\ti")
        );
    }

    #[test]
    fn unicode转义与代理对() {
        // \u002f == '/'：sed 正则正是栽在这里（匹配不到 .comments.md）
        let v = parse(r#"{"file_path":".gates\u002fREQ-001.comments.md"}"#).unwrap();
        assert_eq!(
            v.get("file_path").unwrap().as_str(),
            Some(".gates/REQ-001.comments.md")
        );
        // 代理对：emoji
        let v = parse(r#"{"s":"\uD83D\uDE00"}"#).unwrap();
        assert_eq!(v.get("s").unwrap().as_str(), Some("\u{1F600}"));
        // 中文原文（多字节 UTF-8 直写）
        let v = parse(r#"{"s":"需求清单"}"#).unwrap();
        assert_eq!(v.get("s").unwrap().as_str(), Some("需求清单"));
    }

    #[test]
    fn 非法输入_返回空而非乱猜() {
        assert!(parse("").is_none());
        assert!(parse("{").is_none());
        assert!(parse(r#"{"a":}"#).is_none());
        assert!(parse(r#"{"a":"x"}"#).is_some());
        assert!(parse("{\"a\":1} junk").is_none(), "尾随垃圾应判失败");
        assert!(parse(r#"{"a":"\q"}"#).is_none(), "非法转义应判失败");
    }

    #[test]
    fn file_path_优先tool_input再深搜() {
        assert_eq!(
            file_path_of(r#"{"tool_name":"Write","tool_input":{"file_path":"a.rs"}}"#),
            Some("a.rs".to_string())
        );
        // 结构更深/更扁时靠深搜兜底
        assert_eq!(
            file_path_of(r#"{"x":{"y":{"file_path":"b.rs"}}}"#),
            Some("b.rs".to_string())
        );
        // 字符串里带伪装字段：必须取真实值，不能被 "content" 里的文本带偏
        let v =
            parse(r#"{"tool_input":{"content":"file_path\":\"fake.md\"","file_path":"real.md"}}"#);
        assert!(v.is_some(), "转义引号不应破坏解析");
        assert_eq!(
            file_path_of(r#"{"tool_input":{"content":"junk","file_path":"real.md"}}"#),
            Some("real.md".to_string())
        );
        assert!(file_path_of("not json").is_none());
    }
}
