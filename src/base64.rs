//! Base64 encode/decode module.
//!
//! ```lua
//! local b64 = std.base64
//! local encoded = b64.encode("hello")        --> "aGVsbG8="
//! local decoded = b64.decode("aGVsbG8=")     --> "hello"
//! local url_enc = b64.encode_url("hello")    --> "aGVsbG8"
//! local url_dec = b64.decode_url("aGVsbG8")  --> "hello"
//! ```

use mlua::prelude::*;

use base64::Engine;

pub fn module(lua: &Lua) -> LuaResult<LuaTable> {
    let t = lua.create_table()?;

    t.set(
        "encode",
        lua.create_function(|_, s: String| {
            Ok(::base64::engine::general_purpose::STANDARD.encode(s.as_bytes()))
        })?,
    )?;

    t.set(
        "decode",
        lua.create_function(|_, s: String| {
            let bytes = ::base64::engine::general_purpose::STANDARD
                .decode(s.as_bytes())
                .map_err(|e| LuaError::external(format!("base64.decode: {e}")))?;
            String::from_utf8(bytes).map_err(|e| LuaError::external(format!("base64.decode: {e}")))
        })?,
    )?;

    t.set(
        "encode_url",
        lua.create_function(|_, s: String| {
            Ok(::base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_bytes()))
        })?,
    )?;

    t.set(
        "decode_url",
        lua.create_function(|_, s: String| {
            let bytes = ::base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(s.as_bytes())
                .map_err(|e| LuaError::external(format!("base64.decode_url: {e}")))?;
            String::from_utf8(bytes)
                .map_err(|e| LuaError::external(format!("base64.decode_url: {e}")))
        })?,
    )?;

    Ok(t)
}

#[cfg(test)]
mod tests {
    use crate::util::test_eval as eval;

    #[test]
    fn encode_hello() {
        let s: String = eval(r#"return std.base64.encode("hello")"#);
        assert_eq!(s, "aGVsbG8=");
    }

    #[test]
    fn decode_hello() {
        let s: String = eval(r#"return std.base64.decode("aGVsbG8=")"#);
        assert_eq!(s, "hello");
    }

    #[test]
    fn roundtrip() {
        let s: String = eval(
            r#"
            local encoded = std.base64.encode("hello world 🌍")
            return std.base64.decode(encoded)
        "#,
        );
        assert_eq!(s, "hello world 🌍");
    }

    #[test]
    fn encode_empty() {
        let s: String = eval(r#"return std.base64.encode("")"#);
        assert_eq!(s, "");
    }

    #[test]
    fn decode_empty() {
        let s: String = eval(r#"return std.base64.decode("")"#);
        assert_eq!(s, "");
    }

    #[test]
    fn decode_invalid_returns_error() {
        let lua = mlua::Lua::new();
        crate::register_all(&lua, "std").unwrap();
        let result: mlua::Result<mlua::Value> = lua
            .load(r#"return std.base64.decode("!!invalid!!")"#)
            .eval();
        assert!(result.is_err());
    }

    #[test]
    fn encode_url_no_padding() {
        let s: String = eval(r#"return std.base64.encode_url("hello")"#);
        assert_eq!(s, "aGVsbG8");
        assert!(!s.contains('='));
    }

    #[test]
    fn decode_url_no_padding() {
        let s: String = eval(r#"return std.base64.decode_url("aGVsbG8")"#);
        assert_eq!(s, "hello");
    }

    #[test]
    fn url_roundtrip() {
        let s: String = eval(
            r#"
            local encoded = std.base64.encode_url("test+data/here")
            return std.base64.decode_url(encoded)
        "#,
        );
        assert_eq!(s, "test+data/here");
    }

    #[test]
    fn url_safe_chars() {
        let s: String = eval(r#"return std.base64.encode_url("subjects?_d")"#);
        assert!(!s.contains('+'));
        assert!(!s.contains('/'));
    }
}
