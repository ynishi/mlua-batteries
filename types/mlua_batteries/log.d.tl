-- mlua-batteries `log`: the host's `log` facade. The optional table is
-- appended to the message as `key=value` pairs.
local record log
   debug: function(msg: string, data?: {string: any})
   info: function(msg: string, data?: {string: any})
   warn: function(msg: string, data?: {string: any})
   error: function(msg: string, data?: {string: any})
   -- "trace" | "debug" | "info" | "warn" | "error"; raises on any other.
   is_enabled: function(string): boolean
end

return log
