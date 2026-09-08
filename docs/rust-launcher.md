# Shell-free Rust launcher

The runtime builds the canonical `ores-launcher` from ores-otel/ores.otel.log at
the immutable revision in `docker/ores-launcher.rev`, using `cargo install
--locked`. It emits the existing ores-otel JSON command record to stderr, flushes
locally, then uses Unix exec. There is no copied launcher implementation, shell,
remote exporter, environment dump, or additional option parser in this repository.

`ENTRYPOINT ["/ores-launcher", "/app/canonical-api-server", "serve"]` retains the
previous fixed `serve` argument. Arguments after the image name are still appended
to `serve`. To replace the executable or subcommand explicitly:

```sh
docker run --rm --entrypoint /ores-launcher IMAGE /app/canonical-api-server COMMAND
```

Host/platform secret injection remains required. The launcher does not replace
SOPS decryption, an init/reaper, or the application's graceful shutdown. Never pass
credentials in argv: bounded redaction cannot discover arbitrary positional secrets.

Native amd64/arm64 CI builds the actual API image and tests the launcher in it:
exact entrypoint/default-argument/user metadata, exits 64/127, ores-otel stderr
records, literal argv logging and absent sh/bash. Test containers have read-only
root filesystems, no network, no capabilities and no-new-privileges. This smoke
contract does not start the API or certify auth, database access, graceful drain,
or deployment; all existing API and security gates remain required before merge.
