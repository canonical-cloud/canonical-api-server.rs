# Quote database compatibility witnesses

The editable PostgreSQL authority for the quote/readiness plane is
`canonical-cloud/canonical-orm-core/sql/quote/`, not this directory.

These files remain temporarily because existing declarative-postgres CI and
promotion tooling in this repository consume local paths:

- `bootstrap.sql`
- `schema.sql`
- `grants.sql`

Treat them as frozen compatibility witnesses. Do not originate schema, grant,
role, RLS, constraint, index, trigger, or function changes here. Make the change
in `canonical-orm-core`, review/test it there, and then mechanically materialize
or replace the local witness as part of the consumer/deployment cutover.

`namespace.json` records the source repository/path and the transition commit.
The target state is to remove this duplicated material or generate it read-only
from orm-core so there is one persistence authority.
