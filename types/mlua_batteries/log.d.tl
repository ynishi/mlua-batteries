-- mlua-batteries `log`: the host's `log` facade. The optional table is
-- appended to the message as `key=value` pairs.
local record log
   debug: function(string, {string: any})
   info: function(string, {string: any})
   warn: function(string, {string: any})
   error: function(string, {string: any})
   -- "trace" | "debug" | "info" | "warn" | "error"; raises on any other.
   is_enabled: function(string): boolean
end

return log
