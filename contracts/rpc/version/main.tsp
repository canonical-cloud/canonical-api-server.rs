// Independently authored TypeSpec authority for the version RPC semantic payloads.
// JSON Schema peer: authored.schema.json. Generated JSON Schema is comparison evidence only.
import "@typespec/json-schema";
using TypeSpec.JsonSchema;

namespace CanonicalCloud.Rpc.Version;

@jsonSchema
@extension("unevaluatedProperties", false)
model VersionResponse {
  result: {
    service: string;
    version: string;
  };
  traceIds: string[];
}

@jsonSchema
@extension("unevaluatedProperties", false)
model VersionError {
  code: string;
  message: string;
}
