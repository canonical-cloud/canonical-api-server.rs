#!/usr/bin/env python3
from itertools import product


def admitted(authenticated: bool, tenant_bound: bool, schema_valid: bool, idempotency_key: bool) -> bool:
    return authenticated and tenant_bound and schema_valid and idempotency_key


def main() -> None:
    explored = 0
    for authenticated, tenant_bound, schema_valid, idempotency_key in product((False, True), repeat=4):
        explored += 1
        ok = admitted(authenticated, tenant_bound, schema_valid, idempotency_key)
        if ok:
            assert authenticated and tenant_bound and schema_valid and idempotency_key
        else:
            assert not (authenticated and tenant_bound and schema_valid and idempotency_key)
    assert not admitted(True, True, True, False), 'write admitted without idempotency key'
    print(f'privileged write admission: {explored} states')


if __name__ == '__main__':
    main()
