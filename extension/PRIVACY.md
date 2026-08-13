# XTUI Browser Bridge privacy

XTUI Browser Bridge makes read-only X web requests requested by the local XTUI terminal client.
For Home timelines and threads it reads the browser's X CSRF cookie in memory and sends requests
directly from the extension worker; it does not open or scroll an X tab. Cookies are never stored by XTUI,
included in native-messaging responses, or transmitted anywhere except x.com by the browser.

Normalized posts and cursors are kept in extension session storage so browser suspension cannot
reset the current XTUI session. This cache is cleared when XTUI shuts down.

Extracted posts travel only between the extension and the local XTUI native-messaging host. The
extension does not provide posting, liking, following, or direct-message actions. Rendered-page
extraction remains a compatibility path for routes that have not moved to direct transport yet.
