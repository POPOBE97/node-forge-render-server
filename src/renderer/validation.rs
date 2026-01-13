//! WGSL validation using the naga library.

use anyhow::{anyhow, Context, Result};

/// Validate WGSL source code using naga's parser.
///
/// # Arguments
/// * `source` - The WGSL source code to validate
///
/// # Returns
/// The parsed naga Module on success, or an error with detailed information on failure.
///
/// # Example
/// ```ignore
/// let wgsl = "fn main() -> vec4f { return vec4f(1.0); }";
/// match validate_wgsl(wgsl) {
///     Ok(module) => println!("Valid WGSL"),
///     Err(e) => eprintln!("Invalid WGSL: {}", e),
/// }
/// ```
pub fn validate_wgsl(source: &str) -> Result<naga::Module> {
    naga::front::wgsl::parse_str(source)
        .map_err(|e| anyhow!("WGSL validation failed:\n{}", format_naga_error(source, &e)))
}

/// Validate WGSL and provide context about which pass/component generated it.
///
/// # Arguments
/// * `source` - The WGSL source code to validate
/// * `context` - Description of what generated this WGSL (e.g., "pass my_render_pass")
///
/// # Returns
/// The parsed naga Module on success, or an error with context on failure.
pub fn validate_wgsl_with_context(source: &str, context: &str) -> Result<naga::Module> {
    validate_wgsl(source).with_context(|| format!("{} generated invalid WGSL", context))
}

/// Format a naga parse error with source context for better error messages.
///
/// # Arguments
/// * `source` - The WGSL source code that failed to parse
/// * `error` - The naga parse error
///
/// # Returns
/// A formatted string with error details and source context
fn format_naga_error(source: &str, error: &naga::front::wgsl::ParseError) -> String {
    let mut output = String::new();
    
    // Add main error message
    output.push_str(&format!("  {}\n", error));
    
    // Try to add source context if we can extract location info
    // Note: naga's error structure may vary by version
    output.push_str("\nGenerated WGSL:\n");
    output.push_str("---\n");
    
    // Add line numbers to source for easier debugging
    for (line_num, line) in source.lines().enumerate() {
        output.push_str(&format!("{:4} | {}\n", line_num + 1, line));
    }
    output.push_str("---\n");
    
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_wgsl() {
        let source = r#"
@vertex
fn vs_main(@location(0) position: vec3f) -> @builtin(position) vec4f {
    return vec4f(position, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4f {
    return vec4f(1.0, 0.0, 0.0, 1.0);
}
"#;
        assert!(validate_wgsl(source).is_ok());
    }

    #[test]
    fn test_invalid_wgsl_syntax() {
        let source = "fn invalid() -> { return vec4f(1.0); }"; // Missing type
        assert!(validate_wgsl(source).is_err());
    }

    #[test]
    fn test_invalid_wgsl_type_error() {
        let source = r#"
@fragment
fn fs_main() -> @location(0) vec4f {
    let x: vec4f = 1.0; // Type mismatch: assigning f32 to vec4f
    return x;
}
"#;
        assert!(validate_wgsl(source).is_err());
    }

    #[test]
    fn test_validate_with_context() {
        let source = "invalid wgsl";
        let result = validate_wgsl_with_context(source, "test pass");
        assert!(result.is_err());
        let err_msg = format!("{:#}", result.unwrap_err());
        assert!(err_msg.contains("test pass"));
    }
}
