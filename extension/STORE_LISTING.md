# XTUI Browser Bridge

Use the X account already signed in to Edge or Chrome from the XTUI terminal client. The
extension has no browsing interface: it maintains bounded inactive transport tabs for active XTUI
routes and sends rendered, read-only timeline data to the local native-messaging host.

Permissions: `nativeMessaging` connects to the locally installed XTUI binary; `tabs` owns the
inactive transport tabs; `https://x.com/*` is the only site scope. No telemetry, cookie
export, posting, liking, following, or direct-message access is implemented.
