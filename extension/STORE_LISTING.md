# XTUI Browser Bridge

Use the X account already signed in to Edge or Chrome from the XTUI terminal client. The
extension has no browsing interface: Home timelines and threads load directly in the extension worker and
normalized post data is sent to the local native-messaging host. Rendered-card extraction remains
a compatibility fallback for secondary routes.

Permissions: `nativeMessaging` connects to the locally installed XTUI binary; `cookies` reads X's
CSRF cookie in memory for authenticated read-only requests; `storage` preserves normalized posts
across worker suspension for the current XTUI session; `tabs` supports secondary-route
compatibility; `https://x.com/*` is the only site scope. No telemetry, cookie export, posting,
liking, following, or direct-message access is implemented.
