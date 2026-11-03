Placeholder mount target.

`docker-compose.yml` always bind-mounts a certificate directory at `/certs`.
When a goatnet profile leaves `GOATMAP_CERT_DIR` empty (the fabric leg is not
wired yet), compose falls back to this directory so the mount is a real, empty,
read-only directory rather than a missing path — which compose would otherwise
materialise as a root-owned directory, or fail on outright.
