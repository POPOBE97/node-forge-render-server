use anyhow::{Result, anyhow, bail};

use crate::renderer::types::Kernel2D;

pub(crate) fn parse_kernel_source_js_like(source: &str) -> Result<Kernel2D> {
    // Strip JS comments so we don't accidentally match docstrings like "width/height: number".
    fn strip_js_comments(src: &str) -> String {
        // Minimal, non-string-aware comment stripper:
        // - removes // line comments
        // - removes /* block comments */
        let mut out = String::with_capacity(src.len());
        let mut i = 0;
        let bytes = src.as_bytes();
        let mut in_block = false;
        while i < bytes.len() {
            if in_block {
                if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    in_block = false;
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }

            // Block comment start
            if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                in_block = true;
                i += 2;
                continue;
            }
            // Line comment start
            if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
                // Skip until newline
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }

            out.push(bytes[i] as char);
            i += 1;
        }
        out
    }

    let source = strip_js_comments(source);

    // Minimal parser for the editor-authored Kernel node `params.source`.
    // Expected form (JavaScript-like):
    // return { width: 3, height: 3, value: [ ... ] };
    // or: return { width: 3, height: 3, values: [ ... ] };

    fn find_field_after_colon<'a>(src: &'a str, key: &str) -> Result<&'a str> {
        // Find `key` as an identifier (not inside comments like `width/height`) and return the
        // substring after its ':' (trimmed).
        let bytes = src.as_bytes();
        let key_bytes = key.as_bytes();
        'outer: for i in 0..=bytes.len().saturating_sub(key_bytes.len()) {
            if &bytes[i..i + key_bytes.len()] != key_bytes {
                continue;
            }
            // Word boundary before key.
            if i > 0 {
                let prev = bytes[i - 1];
                if prev.is_ascii_alphanumeric() || prev == b'_' {
                    continue;
                }
            }
            // After key: skip whitespace then require ':'
            let mut j = i + key_bytes.len();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= bytes.len() || bytes[j] != b':' {
                continue;
            }
            j += 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            // Ensure this isn't a prefix of a longer identifier.
            if i + key_bytes.len() < bytes.len() {
                let next = bytes[i + key_bytes.len()];
                if next.is_ascii_alphanumeric() || next == b'_' {
                    continue 'outer;
                }
            }
            return Ok(&src[j..]);
        }
        bail!("Kernel.source missing {key}")
    }

    fn parse_u32_field(src: &str, key: &str) -> Result<u32> {
        let after_colon = find_field_after_colon(src, key)?;
        let mut num = String::new();
        for ch in after_colon.chars() {
            if ch.is_ascii_digit() {
                num.push(ch);
            } else {
                break;
            }
        }
        if num.is_empty() {
            bail!("Kernel.source field {key} missing numeric value");
        }
        Ok(num.parse::<u32>()?)
    }

    fn parse_f32_array_field(src: &str, key: &str) -> Result<Vec<f32>> {
        let after_colon = find_field_after_colon(src, key)?;
        let lb = after_colon
            .find('[')
            .ok_or_else(|| anyhow!("Kernel.source missing '[' for {key}"))?;
        let after_lb = &after_colon[lb + 1..];
        let rb = after_lb
            .find(']')
            .ok_or_else(|| anyhow!("Kernel.source missing ']' for {key}"))?;
        let inside = &after_lb[..rb];

        let mut values: Vec<f32> = Vec::new();
        let mut token = String::new();
        for ch in inside.chars() {
            if ch.is_ascii_digit() || ch == '.' || ch == '-' || ch == '+' || ch == 'e' || ch == 'E'
            {
                token.push(ch);
            } else if !token.trim().is_empty() {
                values.push(token.trim().parse::<f32>()?);
                token.clear();
            } else {
                token.clear();
            }
        }
        if !token.trim().is_empty() {
            values.push(token.trim().parse::<f32>()?);
        }
        Ok(values)
    }

    let w = parse_u32_field(source.as_str(), "width")?;
    let h = parse_u32_field(source.as_str(), "height")?;
    // Prefer `values` when present; otherwise fallback to `value`.
    let values = match parse_f32_array_field(source.as_str(), "values") {
        Ok(v) => v,
        Err(_) => parse_f32_array_field(source.as_str(), "value")?,
    };

    let expected = (w as usize).saturating_mul(h as usize);
    if expected == 0 {
        bail!("Kernel.source invalid size: {w}x{h}");
    }
    if values.len() != expected {
        bail!(
            "Kernel.source values length mismatch: expected {expected} for {w}x{h}, got {}",
            values.len()
        );
    }

    Ok(Kernel2D {
        width: w,
        height: h,
        values,
    })
}
