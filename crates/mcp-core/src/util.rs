use crate::json::{obj, Value};

/// Truncate to roughly `max` bytes on a char boundary, keeping head and tail.
pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let head = max * 2 / 3;
    let tail = max - head;
    let mut h = head;
    while !s.is_char_boundary(h) { h -= 1; }
    let mut t = s.len() - tail;
    while !s.is_char_boundary(t) { t += 1; }
    format!("{}\n… [{} bytes omitted] …\n{}", &s[..h], t - h, &s[t..])
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Property spec for `schema`: (name, json type, description).
pub type Prop = (&'static str, &'static str, &'static str);

/// Build a JSON Schema object for tool input.
pub fn schema(props: &[Prop], required: &[&str]) -> Value {
    let p: Vec<(String, Value)> = props
        .iter()
        .map(|(n, t, d)| (n.to_string(), obj([("type", (*t).into()), ("description", (*d).into())])))
        .collect();
    obj([
        ("type", "object".into()),
        ("properties", Value::Obj(p)),
        ("required", Value::Arr(required.iter().map(|r| (*r).into()).collect())),
    ])
}

pub fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn b64() {
        assert_eq!(super::base64(b"hola"), "aG9sYQ==");
        assert_eq!(super::base64(b"abc"), "YWJj");
    }
}
