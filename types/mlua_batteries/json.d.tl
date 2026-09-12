-- mlua-batteries `json`: JSON encode / decode (serde_json).
--
-- Every function raises on failure (malformed input, unsupported value,
-- nesting deeper than the host's max_json_depth). To get a result instead,
-- wrap the call:
--
--    local ok, v = pcall(json.decode, text)          -- v: any
--    local ok, r = pcall(function(): R return json.decode(text) end)
--
-- `decode` / `read_file` are declared twice: once returning `any`, so a
-- bare call or one passed to pcall type-checks, and once as `<T>`, so an
-- annotated `local r: R = json.decode(text)` gets its type without a cast.
--
-- Empty tables encode as `{}`; use `array()` (or a metatable with
-- `__jsontype = "array"`) for `[]`. `decode` lowers JSON null to nil; to
-- *emit* an explicit null put `null` in the value.
local record json
   -- The encoder's null sentinel: an opaque userdata, so a field that may
   -- carry it is declared `T | json.Null`.
   type Null = userdata

   null: Null
   is_null: function(any): boolean

   encode: function(any): string
   encode_pretty: function(any): string
   decode: function(string): any
   decode: function<T>(string): T

   -- An empty table tagged to encode as `[]`.
   array: function<T>(): {T}

   read_file: function(string): any
   read_file: function<T>(string): T
   write_file: function(string, any): boolean
end

return json
