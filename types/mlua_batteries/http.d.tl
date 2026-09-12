-- mlua-batteries `http`: blocking HTTP client (ureq) under the host's
-- HTTP policy. A denied URL, a transport failure or a body over the
-- host's max_response_bytes raises; a non-2xx status does not.
local record http
   record Response
      status: integer
      body: string
   end

   -- Built by the caller: method and url are required, the rest optional.
   record Request   ---@struct
      method: string
      url: string
      headers: {string: string}   ---@optional
      body: string                ---@optional
      -- Seconds; the host's default when absent.
      timeout: integer            ---@optional
      content_type: string        ---@optional
   end

   get: function(string): Response
   -- post(url, body, content_type?); content_type defaults to
   -- application/json.
   post: function(url: string, body: string, content_type?: string): Response
   request: function(Request): Response
end

return http
