use apps_runtime::{
    host::{Host, Role},
    jsc::{Api, Engine},
};
use std::{cell::RefCell, rc::Rc};

#[test]
fn sqlite_statements_values_and_lifecycle() {
    let engine = Engine::new(
        Api::load().unwrap(),
        Rc::new(RefCell::new(Host::default())),
        Role::Main,
    )
    .unwrap();
    engine.eval(r#"
        const { DatabaseSync, StatementSync } = __apps.sqlite;
        const assert = (value, message) => { if (!value) throw Error(message); };
        const throws = (fn, message) => {
            let failed = false;
            try { fn(); } catch { failed = true; }
            assert(failed, message);
        };
        throws(() => new StatementSync(), 'Statement constructor');
        throws(() => new DatabaseSync(':memory:', { timeout: -1 }), 'Invalid timeout');
        throws(() => new DatabaseSync(':memory:', { readOnly: 1 }), 'Invalid boolean');
        throws(() => new DatabaseSync(':memory:', { allowExtension: true }), 'Extensions');
        const db = new DatabaseSync(':memory:', { open: false });
        assert(!db.isOpen && !db.isTransaction, 'Deferred open');
        throws(() => db.prepare('SELECT 1'), 'Prepare while closed');
        db.open();
        throws(() => db.open(), 'Double open');
        assert(db.isOpen, 'Open state');
        db.exec('CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT, data BLOB, value INTEGER)');
        const insert = db.prepare('INSERT INTO items (name, data, value) VALUES (?, ?, ?)');
        assert(insert instanceof StatementSync, 'Statement identity');
        const bytes = new Uint8Array([9, 0, 255, 9]);
        let result = insert.run('one\0🌊', bytes.subarray(1, 3), 42n);
        assert(result.changes === 1 && result.lastInsertRowid === 1, 'Run result');
        insert.run('two', new DataView(bytes.buffer, 2, 1), null);
        insert.run('three', new Uint8Array(), 9223372036854775807n);
        const select = db.prepare('SELECT * FROM items WHERE id = ?');
        const row = select.get(1);
        assert(Object.getPrototypeOf(row) === null, 'Row prototype');
        assert(row.name === 'one\0🌊' && row.value === 42, 'Text and integer');
        assert(row.data instanceof Uint8Array && row.data.join(',') === '0,255', 'Blob offset');
        assert(select.get(2).data.join(',') === '255' && select.get(2).value === null, 'DataView and null');
        throws(() => select.get(3), 'Unsafe integer');
        select.setReadBigInts(true);
        assert(select.get(3).value === 9223372036854775807n && select.get(3).data.length === 0, 'BigInt and empty blob');
        select.setReadBigInts(false);
        assert(select.get(100) === undefined && select.all(100).length === 0, 'Empty results');
        assert(select.get() === undefined, 'Cleared bindings');
        assert(select.sourceSQL.includes('WHERE id = ?'), 'Source SQL');
        select.get(1);
        assert(select.expandedSQL.includes('WHERE id = 1'), 'Expanded SQL');
        const named = db.prepare('SELECT $name AS name, :value AS value, ? AS positional');
        assert(named.get({ name: 'a', ':value': 2 }, 3).positional === 3, 'Mixed bindings');
        throws(() => named.get({ missing: 1 }), 'Unknown named parameter');
        named.setAllowUnknownNamedParameters(true);
        assert(named.get({ missing: 1 }).name === null, 'Ignored unknown parameter');
        named.setAllowUnknownNamedParameters(false);
        named.setAllowBareNamedParameters(false);
        throws(() => named.get({ name: 'a' }), 'Bare parameters disabled');
        assert(named.get({ $name: 'b' }).name === 'b', 'Prefixed parameter');
        throws(() => db.prepare('SELECT $x, @x').get({ x: 1 }), 'Ambiguous parameter');
        throws(() => select.get(1, 2), 'Excess parameters');
        throws(() => insert.run('bad', null, 9223372036854775808n), 'Out of range BigInt');
        throws(() => insert.run('bad', null, {}), 'Invalid parameter');
        const special = db.prepare('SELECT ? AS x');
        assert(special.get(NaN).x === null && special.get(Infinity).x === Infinity && special.get(-Infinity).x === -Infinity, 'Non-finite numbers');
        assert(special.get(true).x === 1 && special.get(undefined).x === null, 'Boolean and undefined');
        assert(special.get(bytes.buffer).x.length === 4, 'ArrayBuffer');
        assert(db.prepare('SELECT 1 AS __proto__').get().__proto__ === 1, 'Prototype column');
        assert(db.prepare('SELECT ?1 AS a, ?1 AS b').get({ '1': 4 }).b === 4, 'Numbered parameter');
        assert(db.prepare('SELECT ?1 AS a, ?1 AS b').get(4).b === 4, 'Positional numbered parameter');
        assert(db.prepare('SELECT ?3 AS a').get(1, 2, 3).a === 3, 'Numbered parameter gaps');
        select.setReturnArrays(true);
        assert(Array.isArray(select.get(1)) && select.get(1)[0] === 1, 'Array rows');
        insert.setReadBigInts(true);
        result = insert.run('four', null, -9223372036854775808n);
        assert(result.changes === 1n && result.lastInsertRowid === 4n, 'BigInt run result');
        db.exec('BEGIN; INSERT INTO items (name) VALUES (\'rollback\')');
        assert(db.isTransaction, 'Transaction state');
        db.exec('ROLLBACK');
        assert(!db.isTransaction && db.prepare('SELECT count(*) AS n FROM items').get().n === 4, 'Rollback');
        db.exec('CREATE TABLE children (parent INTEGER REFERENCES items(id))');
        throws(() => db.exec('INSERT INTO children VALUES (100)'), 'Foreign keys enabled');
        throws(() => db.prepare('SELECT "not_a_column"'), 'Double quoted strings disabled');
        throws(() => db.prepare('not valid SQL'), 'Invalid SQL');
        throws(() => db.prepare(''), 'Empty SQL');
        db.exec('CREATE TABLE changes (id INTEGER PRIMARY KEY)');
        const returning = db.prepare('INSERT INTO changes VALUES (?),(?) RETURNING id');
        result = returning.run(1, 2);
        assert(result.changes === 2 && result.lastInsertRowid === 2, 'Returning change count');
        throws(() => returning.run(1, 2), 'Constraint error');
        assert(returning.run(3, 4).changes === 2, 'Reuse after constraint error');
        const schema = db.prepare('SELECT * FROM changes');
        db.exec('ALTER TABLE changes ADD COLUMN name TEXT');
        assert(schema.get().name === null, 'Schema reprepare');
        select.close();
        select.close();
        throws(() => select.get(1), 'Closed statement');
        db.close();
        assert(!db.isOpen, 'Closed state');
        throws(() => insert.run('five', null, 1), 'Closed database statement');
        throws(() => db.close(), 'Double close');
        db.open();
        throws(() => insert.run('five', null, 1), 'Stale statement after reopen');
        db.close();
        const reopened = new DatabaseSync(':memory:');
        const all = new Uint8Array(256).map((_, i) => i);
        const values = reopened.prepare('SELECT ? AS b, 9007199254740991 AS i, 1e300 AS r, -0.0 AS z, 9e999 AS inf').get(all);
        assert(values.b.join() === all.join() && values.i === 9007199254740991 && values.r === 1e300 && Object.is(values.z, -0) && values.inf === Infinity, 'Value transfer');
        reopened.setAuthorizer(() => { reopened.prepare('SELECT 1'); return 0; });
        throws(() => reopened.prepare('SELECT 2'), 'Reentrant authorizer');
        reopened.setAuthorizer(null);
        assert(reopened.prepare('SELECT 3 AS x').get().x === 3, 'Usable after authorizer');
        reopened.close();
        const configured = new DatabaseSync(':memory:', { readBigInts: true, returnArrays: true, enableForeignKeyConstraints: false, enableDoubleQuotedStringLiterals: true });
        assert(configured.prepare('SELECT 1, "text"').get().join(',') === '1,text', 'Database defaults');
        assert(configured.prepare('PRAGMA foreign_keys').get()[0] === 0n, 'Foreign keys disabled');
        configured.close();
    "#, "sqlite-test").unwrap();
}

#[test]
fn sqlite_iterators_release_statements_and_read_incrementally() {
    let engine = Engine::new(
        Api::load().unwrap(),
        Rc::new(RefCell::new(Host::default())),
        Role::Preload(1),
    )
    .unwrap();
    engine.eval(r#"
        const assert = (value, message) => { if (!value) throw Error(message); };
        const db = new __apps.sqlite.DatabaseSync(':memory:');
        const query = db.prepare('WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 1000000000) SELECT x FROM n');
        const iterator = query.iterate();
        assert(iterator[Symbol.iterator]() === iterator, 'Iterable iterator');
        assert(iterator.next().value.x === 1 && iterator.next().value.x === 2, 'Incremental reads');
        iterator.return();
        assert(iterator.next().done && query.get().x === 1, 'Return resets statement');
        const invalidated = query.iterate();
        invalidated.next();
        query.get();
        let rejected = false;
        try { invalidated.next(); } catch { rejected = true; }
        assert(rejected, 'Statement reuse invalidates iterator');
        for (const row of query.iterate()) { assert(row.x === 1, 'First row'); break; }
        assert(query.get().x === 1, 'Break resets statement');
        const small = db.prepare('SELECT 1 AS x UNION ALL SELECT 2');
        assert([...small.iterate()].map(row => row.x).join(',') === '1,2', 'Iteration results');
        const drained = small.iterate();
        drained.next(); drained.next();
        assert(drained.next().done && drained.next().done, 'Drained iterator stays done');
        const big = db.prepare('SELECT 9223372036854775807 AS x');
        rejected = false;
        try { big.iterate().next(); } catch { rejected = true; }
        assert(rejected, 'Iterator conversion error');
        big.setReadBigInts(true);
        assert(big.get().x === 9223372036854775807n, 'Error releases iterator');
        const closed = query.iterate();
        query.close();
        assert(closed.next().done, 'Statement close ends iterator');
        db.close();
    "#, "sqlite-iterator-test").unwrap();
}
