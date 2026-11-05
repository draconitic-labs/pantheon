# Security

Pantheon is a local desktop app with no accounts. The atlas makes no network connections.

The 3D window runs a small HTTP server on 127.0.0.1 for its own page. It serves only the page's files, and refuses requests with another `Host` or `Origin`. `tools/fetch_data.py` downloads the public datasets listed in `NOTICE` over HTTPS, and only when you run it.

To report a problem, use "Report a vulnerability" on the repository's Security tab if it is available. Otherwise open an issue that describes the affected area without exploit details, and ask for a private way to send them.
