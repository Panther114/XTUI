# XTUI Browser Bridge privacy

XTUI Browser Bridge reads the rendered X pages requested by the local XTUI terminal client.
It does not request cookie access, export browser sessions, transmit data to an XTUI server,
or provide write actions on X. Extracted posts travel only between the extension and the local
XTUI native-messaging host. Inactive, muted transport tabs are isolated by active route so opening
a thread cannot stop timeline loading; they exist only while XTUI is active and close when the
session ends.
