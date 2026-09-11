-- mlua-batteries `time`: timestamps, sleep and measurement.
local record time
   -- Unix epoch seconds, fractional.
   now: function(): number
   -- Unix epoch milliseconds.
   millis: function(): integer
   -- Blocks the VM thread (async under `async_overrides`).
   sleep: function(number)
   -- Elapsed seconds, then whatever the function returned.
   measure: function(function(): any...): number, any...
end

return time
