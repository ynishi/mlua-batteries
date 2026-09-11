-- mlua-batteries `http`: blocking HTTP client (ureq) under the host's
-- HTTP policy. A denied URL, a transport failure or a body over the
-- host's max_response_bytes raises; a non-2xx status does not.
local record http
   record Response
      status: integer
      body: string
   end

   record Request
      method: string
      url: string
      headers: {string: string}
      body: string
      -- Seconds; the host's default when absent.
      timeout: integer
      content_type: string
   end

   get: function(string): Response
   -- post(url, body, content_type?); content_type defaults to
   -- application/json.
   post: function(string, string, string): Response
   request: function(Request): Response
end

return http
