// Independently authored TypeSpec authority for Canonical regular RPC semantic payloads.
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

@jsonSchema
@extension("unevaluatedProperties", false)
model GetUserByIdHeaders {
  @minLength(8)
  @maxLength(8199)
  authorization: string;
}

@jsonSchema
@extension("unevaluatedProperties", false)
model GetUserByIdRequest {
  @minLength(1)
  @maxLength(256)
  user_id: string;
}

@jsonSchema
@extension("unevaluatedProperties", false)
model GetUserByIdResponse {
  result: {
    id: string;
    user_name?: string;
    display_name?: string;
    active?: boolean;
  };
  traceIds: string[];
}

@jsonSchema
@extension("unevaluatedProperties", false)
model GetUserByIdError {
  code: string;
  message: string;
}
