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
  authorization: string;
}

@jsonSchema
@extension("unevaluatedProperties", false)
model GetUserByIdRequest {
  user_id: string;
}

@jsonSchema
@extension("unevaluatedProperties", false)
model UserSummary {
  id: string;
  user_name?: string;
  display_name?: string;
  active?: boolean;
}

@jsonSchema
@extension("unevaluatedProperties", false)
model GetUserByIdResponse {
  result: UserSummary;
  traceIds: string[];
}

@jsonSchema
@extension("unevaluatedProperties", false)
model GetUserByIdError {
  code: string;
  message: string;
}
