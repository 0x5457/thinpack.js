(() => {
  const { ops } = Deno.core;
  const { codes } = require("node:thinpack/internal/errors");
  const { toPathIfFileURL } = require("node:thinpack/internal/url");
  const { TextDecoder, TextEncoder } = require("node:thinpack/internal/encoding");
  const constants = {
    SQLITE_CHANGESET_OMIT: 0,
    SQLITE_CHANGESET_REPLACE: 1,
    SQLITE_CHANGESET_ABORT: 2,
    SQLITE_CHANGESET_DATA: 1,
    SQLITE_CHANGESET_NOTFOUND: 2,
    SQLITE_CHANGESET_CONFLICT: 3,
    SQLITE_CHANGESET_CONSTRAINT: 4,
    SQLITE_CHANGESET_FOREIGN_KEY: 5,
    SQLITE_OK: 0,
    SQLITE_DENY: 1,
    SQLITE_IGNORE: 2,
    SQLITE_CREATE_INDEX: 1,
    SQLITE_CREATE_TABLE: 2,
    SQLITE_CREATE_TEMP_INDEX: 3,
    SQLITE_CREATE_TEMP_TABLE: 4,
    SQLITE_CREATE_TEMP_TRIGGER: 5,
    SQLITE_CREATE_TEMP_VIEW: 6,
    SQLITE_CREATE_TRIGGER: 7,
    SQLITE_CREATE_VIEW: 8,
    SQLITE_DELETE: 9,
    SQLITE_DROP_INDEX: 10,
    SQLITE_DROP_TABLE: 11,
    SQLITE_DROP_TEMP_INDEX: 12,
    SQLITE_DROP_TEMP_TABLE: 13,
    SQLITE_DROP_TEMP_TRIGGER: 14,
    SQLITE_DROP_TEMP_VIEW: 15,
    SQLITE_DROP_TRIGGER: 16,
    SQLITE_DROP_VIEW: 17,
    SQLITE_INSERT: 18,
    SQLITE_PRAGMA: 19,
    SQLITE_READ: 20,
    SQLITE_SELECT: 21,
    SQLITE_TRANSACTION: 22,
    SQLITE_UPDATE: 23,
    SQLITE_ATTACH: 24,
    SQLITE_DETACH: 25,
    SQLITE_ALTER_TABLE: 26,
    SQLITE_REINDEX: 27,
    SQLITE_ANALYZE: 28,
    SQLITE_CREATE_VTABLE: 29,
    SQLITE_DROP_VTABLE: 30,
    SQLITE_FUNCTION: 31,
    SQLITE_SAVEPOINT: 32,
    SQLITE_COPY: 0,
    SQLITE_RECURSIVE: 33,
  };
  const NULL = 0;
  const INTEGER = 1;
  const REAL = 2;
  const TEXT = 3;
  const BLOB = 4;

  const MAX_SAFE_INTEGER = 9007199254740991n;
  const MIN_INT64 = -9223372036854775808n;
  const MAX_INT64 = 9223372036854775807n;
  const ABSENT = 0xffffffff;

  const encoder = new TextEncoder();
  const decoder = new TextDecoder();


  const invalidState = (message) => {
    const error = new Error(message);
    error.code = "ERR_INVALID_STATE";
    return error;
  };

  const invalidArgumentValue = (message) => {
    const error = new TypeError(message);
    error.code = "ERR_INVALID_ARG_VALUE";
    return error;
  };
  const sqliteError = (error) => {
    const [code, message, errcode, errstr] = String(error.message).split("\0");
    if (message === undefined) return error;
    const failure = new Error(message);
    failure.code = code;
    if (errcode !== undefined) {
      failure.errcode = Number(errcode);
      failure.errstr = errstr;
    }
    return failure;
  };
  const CALLBACK_ERROR = "ERR_SQLITE_CALLBACK";

  const runOp = (op) => {
    try {
      return op();
    } catch (error) {
      const failure = sqliteError(error);
      if (failure.code === CALLBACK_ERROR) {
        const pending = ops.op_sqlite_take_callback_error();
        if (pending !== undefined) throw pending;
      }
      throw failure;
    }
  };

  const toNumber = (integer) => {
    if (integer > MAX_SAFE_INTEGER || integer < -MAX_SAFE_INTEGER) {
      const error = new RangeError(
        "Value is too large to be represented as a JavaScript number: " + integer,
      );
      error.code = "ERR_OUT_OF_RANGE";
      throw error;
    }
    return Number(integer);
  };

  const booleanOption = (options, name, fallback) => {
    const value = options[name];
    if (value === undefined) return fallback;
    if (typeof value !== "boolean") {
      throw new codes.ERR_INVALID_ARG_TYPE("options." + name, "boolean", value);
    }
    return value;
  };

  const integerOption = (options, name, fallback) => {
    const value = options[name];
    if (value === undefined) return fallback;
    if (!Number.isInteger(value)) {
      throw new codes.ERR_INVALID_ARG_TYPE("options." + name, "integer", value);
    }
    if (value < 0) {
      throw new codes.ERR_OUT_OF_RANGE("options." + name, ">= 0", value);
    }
    return value;
  };

  const databasePath = (path) => {
    const value = toPathIfFileURL(path);
    if (typeof value === "string" && !value.includes("\0")) return value;
    if (ArrayBuffer.isView(value)) return decoder.decode(value);
    throw new codes.ERR_INVALID_ARG_TYPE(
      "path",
      "string, Uint8Array, or URL without null bytes",
      path,
    );
  };
  const blobBytes = (value) => {
    const shared = typeof SharedArrayBuffer === "function" &&
      value.buffer instanceof SharedArrayBuffer;
    if (ArrayBuffer.isView(value)) {
      const view = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
      return shared ? view.slice() : view;
    }
    return new Uint8Array(value);
  };

  const unbindable = (index) => {
    const error = new TypeError(
      "Provided value cannot be bound to SQLite parameter " + index + ".",
    );
    error.code = "ERR_INVALID_ARG_TYPE";
    return error;
  };

  const decodeColumns = (bytes) => {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const count = view.getUint32(0, true);
    const columns = new Array(count);
    let offset = 4;
    const read = () => {
      const length = view.getUint32(offset, true);
      offset += 4;
      if (length === ABSENT) return null;
      const text = decoder.decode(bytes.subarray(offset, offset + length));
      offset += length;
      return text;
    };
    for (let index = 0; index < count; index += 1) {
      const column = Object.create(null);
      column.column = read();
      column.database = read();
      column.name = read();
      column.table = read();
      column.type = read();
      columns[index] = column;
    }
    return columns;
  };

  const iteration = (done, value) => {
    const result = Object.create(null);
    result.done = done;
    result.value = value;
    return result;
  };

  const kStatement = Symbol("kStatement");
  const kDatabase = Symbol("kDatabase");
  const kAllowExtension = Symbol("kAllowExtension");
  const kSession = Symbol("kSession");

  class Database {
    #handle = 0;
    #path;
    #options;

    constructor(path, options = {}) {
      if (options === null || typeof options !== "object") {
        throw new codes.ERR_INVALID_ARG_TYPE("options", "object", options);
      }
      this.#path = databasePath(path);
      this.#options = {
        open: booleanOption(options, "open", true),
        readOnly: booleanOption(options, "readOnly", false),
        enableForeignKeyConstraints: booleanOption(options, "enableForeignKeyConstraints", true),
        enableDoubleQuotedStringLiterals: booleanOption(
          options,
          "enableDoubleQuotedStringLiterals",
          false,
        ),
        defensive: booleanOption(options, "defensive", true),
        timeout: integerOption(options, "timeout", 0),
        readBigInts: booleanOption(options, "readBigInts", false),
        returnArrays: booleanOption(options, "returnArrays", false),
        allowBareNamedParameters: booleanOption(options, "allowBareNamedParameters", true),
        allowUnknownNamedParameters: booleanOption(options, "allowUnknownNamedParameters", false),
        allowExtension: booleanOption(options, "allowExtension", false),
        limits: options.limits,
      };
      if (options.limits !== undefined) {
        if (options.limits === null || typeof options.limits !== "object") {
          throw new codes.ERR_INVALID_ARG_TYPE("options.limits", "object", options.limits);
        }
      }
      if (this.#options.open) this.open();
    }

    get isOpen() {
      return this.#handle !== 0;
    }

    get isTransaction() {
      this.#requireOpen();
      return !runOp(() => ops.op_sqlite_is_autocommit(this.#handle));
    }

    #requireOpen() {
      if (this.#handle === 0) throw invalidState("database is not open");
    }

    open() {
      if (this.#handle !== 0) throw invalidState("database is already open");
      const options = this.#options;
      this.#handle = runOp(() =>
        ops.op_sqlite_open(
          this.#path,
          options.readOnly,
          options.enableForeignKeyConstraints,
          options.enableDoubleQuotedStringLiterals,
          options.defensive,
          options.allowExtension,
          options.timeout,
        )
      );
      for (const [name, value] of Object.entries(this.#options.limits ?? {})) {
        this.limits[name] = value;
      }
    }

    close() {
      this.#requireOpen();
      const handle = this.#handle;
      runOp(() => ops.op_sqlite_close(handle));
      this.#handle = 0;
    }

    [kDatabase]() {
      this.#requireOpen();
      return this.#handle;
    }

    [kAllowExtension]() {
      return this.#options.allowExtension;
    }

    exec(sql) {
      if (typeof sql !== "string") {
        throw new codes.ERR_INVALID_ARG_TYPE("sql", "string", sql);
      }
      this.#requireOpen();
      runOp(() => ops.op_sqlite_exec(this.#handle, sql));
    }

    prepare(sql, options = {}) {
      if (typeof sql !== "string") {
        throw new codes.ERR_INVALID_ARG_TYPE("sql", "string", sql);
      }
      if (options === null || typeof options !== "object") {
        throw new codes.ERR_INVALID_ARG_TYPE("options", "object", options);
      }
      this.#requireOpen();
      const defaults = this.#options;
      const statementOptions = {
        readBigInts: booleanOption(options, "readBigInts", defaults.readBigInts),
        returnArrays: booleanOption(options, "returnArrays", defaults.returnArrays),
        allowBareNamedParameters: booleanOption(
          options,
          "allowBareNamedParameters",
          defaults.allowBareNamedParameters,
        ),
        allowUnknownNamedParameters: booleanOption(
          options,
          "allowUnknownNamedParameters",
          defaults.allowUnknownNamedParameters,
        ),
      };
      const persistent = booleanOption(options, "persistent", false);
      const handle = runOp(() => ops.op_sqlite_prepare(this.#handle, sql, persistent));
      return new Statement(kStatement, handle, sql, statementOptions);
    }

    [Symbol.dispose]() {
      this.close();
    }
  }

  class Statement {
    #handle = 0;
    #sql;
    #readBigInts;
    #returnArrays;
    #allowBareNamedParameters;
    #allowUnknownNamedParameters;
    #parameters = null;
    #bare = null;
    #columns = null;
    #names = null;

    constructor(token, handle, sql, options) {
      if (token !== kStatement) throw new codes.ERR_ILLEGAL_CONSTRUCTOR();
      this.#handle = handle;
      this.#sql = sql;
      this.#readBigInts = options.readBigInts;
      this.#returnArrays = options.returnArrays;
      this.#allowBareNamedParameters = options.allowBareNamedParameters;
      this.#allowUnknownNamedParameters = options.allowUnknownNamedParameters;
    }

    get sourceSQL() {
      this.#requireStatement();
      return this.#sql;
    }

    get expandedSQL() {
      this.#requireStatement();
      return runOp(() => ops.op_sqlite_expanded_sql(this.#handle));
    }

    all(...parameters) {
      this.#execute(parameters);
      const rows = [];
      let row = this.#advance();
      while (row !== undefined) {
        rows.push(row);
        row = this.#advance();
      }
      this.#reset();
      return rows;
    }

    get(...parameters) {
      this.#execute(parameters);
      const row = this.#advance();
      this.#reset();
      return row;
    }

    run(...parameters) {
      this.#execute(parameters);
      let row = this.#advance();
      while (row !== undefined) row = this.#advance();
      const summary = runOp(() => ops.op_sqlite_changes(this.#handle));
      const view = new DataView(summary.buffer, summary.byteOffset, summary.byteLength);
      const changes = view.getBigInt64(0, true);
      const lastInsertRowid = view.getBigInt64(8, true);
      this.#reset();
      return {
        changes: this.#readBigInts ? changes : toNumber(changes),
        lastInsertRowid: this.#readBigInts ? lastInsertRowid : toNumber(lastInsertRowid),
      };
    }

    iterate(...parameters) {
      this.#execute(parameters);
      let finished = false;
      const finish = () => {
        if (finished) return;
        finished = true;
        this.#reset();
      };
      return {
        next: () => {
          if (finished) return iteration(true, null);
          let row;
          try {
            row = this.#advance();
          } catch (error) {
            finished = true;
            throw error;
          }
          if (row === undefined) {
            finish();
            return iteration(true, null);
          }
          return iteration(false, row);
        },
        return: () => {
          finish();
          return iteration(true, null);
        },
        [Symbol.iterator]() {
          return this;
        },
      };
    }

    columns() {
      this.#requireStatement();
      return this.#columnMetadata().map((column) => Object.assign(Object.create(null), column));
    }


    close() {
      this.#requireStatement();
      const handle = this.#handle;
      runOp(() => ops.op_sqlite_finalize(handle));
      this.#handle = 0;
    }

    setReadBigInts(enabled) {
      this.#requireBoolean("readBigInts", enabled);
      this.#requireStatement();
      this.#readBigInts = enabled;
    }

    setReturnArrays(enabled) {
      this.#requireBoolean("returnArrays", enabled);
      this.#requireStatement();
      this.#returnArrays = enabled;
    }

    setAllowBareNamedParameters(enabled) {
      this.#requireBoolean("allowBareNamedParameters", enabled);
      this.#requireStatement();
      this.#allowBareNamedParameters = enabled;
      this.#bare = null;
    }

    setAllowUnknownNamedParameters(enabled) {
      this.#requireBoolean("allowUnknownNamedParameters", enabled);
      this.#requireStatement();
      this.#allowUnknownNamedParameters = enabled;
    }

    [Symbol.dispose]() {
      if (this.#handle !== 0) this.close();
    }

    #requireBoolean(name, value) {
      if (typeof value !== "boolean") {
        throw new codes.ERR_INVALID_ARG_TYPE(name, "boolean", value);
      }
    }

    [kStatement]() {
      this.#requireStatement();
      return this.#handle;
    }

    #requireStatement() {
      if (this.#handle === 0) throw invalidState("statement has been finalized");
    }

    #reset() {
      runOp(() => ops.op_sqlite_reset(this.#handle));
    }

    #execute(parameters) {
      this.#requireStatement();
      runOp(() => ops.op_sqlite_reset(this.#handle));
      this.#bindParameters(parameters);
    }

    #advance() {
      const row = runOp(() => ops.op_sqlite_step(this.#handle));
      return row.length === 0 ? undefined : this.#decodeRow(row);
    }

    #parameterNames() {
      if (this.#parameters === null) {
        this.#requireStatement();
        const fields = runOp(() => ops.op_sqlite_parameter_names(this.#handle)).split("\0");
        const count = Number(fields[0]);
        const names = new Array(count);
        for (let index = 0; index < count; index += 1) {
          names[index] = fields[index + 1] || "";
        }
        this.#parameters = names;
      }
      return this.#parameters;
    }
    #parameterIndex(key) {
      const names = this.#parameterNames();
      const exact = names.findIndex((name) => name !== "" && name === key);
      if (exact !== -1) return exact + 1;
      if (/^[0-9]+$/.test(key)) {
        const numbered = names.findIndex((name) => name === "?" + key);
        if (numbered !== -1) return numbered + 1;
      }
      return 0;
    }
    #bareNameMap() {
      if (this.#bare === null) {
        const bare = new Map();
        for (const name of this.#parameterNames()) {
          if (name === "") continue;
          const bareName = name.slice(1);
          const existing = bare.get(bareName);
          if (existing !== undefined && existing !== name) {
            throw invalidState(
              "Cannot create bare named parameter '" + bareName +
                "' because of conflicting names '" + existing + "' and '" + name + "'.",
            );
          }
          bare.set(bareName, name);
        }
        this.#bare = bare;
      }
      return this.#bare;
    }

    #namedParameterIndex(key) {
      const index = this.#parameterIndex(key);
      if (index !== 0 || !this.#allowBareNamedParameters) return index;
      const name = this.#bareNameMap().get(key);
      return name === undefined ? 0 : this.#parameterIndex(name);
    }
    #bind(index, value) {
      const handle = this.#handle;
      const type = typeof value;
      if (type === "number") {
        runOp(() => ops.op_sqlite_bind_real(handle, index, value));
      } else if (type === "string") {
        runOp(() => ops.op_sqlite_bind_text(handle, index, value));
      } else if (value === null || value === undefined) {
        runOp(() => ops.op_sqlite_bind_null(handle, index));
      } else if (ArrayBuffer.isView(value) || value instanceof ArrayBuffer ||
          (typeof SharedArrayBuffer === "function" && value instanceof SharedArrayBuffer)) {
        runOp(() => ops.op_sqlite_bind_blob(handle, index, blobBytes(value)));
      } else if (type === "boolean") {
        runOp(() => ops.op_sqlite_bind_integer(handle, index, value ? 1n : 0n));
      } else if (type === "bigint") {
        if (value < MIN_INT64 || value > MAX_INT64) {
          throw invalidArgumentValue("BigInt value is too large to bind.");
        }
        runOp(() => ops.op_sqlite_bind_integer(handle, index, value));
      } else {
        throw unbindable(index);
      }
    }

    #bindParameters(parameters) {
      runOp(() => ops.op_sqlite_clear_bindings(this.#handle));
      let anonymousStart = 0;
      const first = parameters[0];
      if (first !== null && typeof first === "object" && !ArrayBuffer.isView(first) &&
          !(first instanceof ArrayBuffer) &&
          !(typeof SharedArrayBuffer === "function" && first instanceof SharedArrayBuffer)) {
        for (const key of Object.getOwnPropertyNames(first)) {
          const index = this.#namedParameterIndex(key);
          if (index === 0) {
            if (this.#allowUnknownNamedParameters) continue;
            throw invalidState("Unknown named parameter '" + key + "'");
          }
          this.#bind(index, first[key]);
        }
        anonymousStart = 1;
      }

      const names = this.#parameterNames();
      let anonymousIndex = 1;
      for (let position = anonymousStart; position < parameters.length; position += 1) {
        while (anonymousIndex <= names.length) {
          const name = names[anonymousIndex - 1];
          if (name === "" || name[0] === "?") break;
          anonymousIndex += 1;
        }
        this.#bind(anonymousIndex, parameters[position]);
        anonymousIndex += 1;
      }
    }

    #columnMetadata() {
      if (this.#columns === null) {
        this.#requireStatement();
        this.#columns = decodeColumns(runOp(() => ops.op_sqlite_columns(this.#handle)));
      }
      return this.#columns;
    }


    #columnNames() {
      if (this.#names === null) {
        this.#names = this.#columnMetadata().map((column) => column.name);
      }
      return this.#names;
    }

    #decodeRow(bytes) {
      const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
      const count = view.getUint32(0, true);
      const names = this.#returnArrays ? null : this.#columnNames();
      const row = this.#returnArrays ? new Array(count) : Object.create(null);
      let offset = 4;
      for (let index = 0; index < count; index += 1) {
        const kind = bytes[offset];
        offset += 1;
        let value;
        if (kind === INTEGER) {
          const integer = view.getBigInt64(offset, true);
          offset += 8;
          value = this.#readBigInts ? integer : toNumber(integer);
        } else if (kind === REAL) {
          value = view.getFloat64(offset, true);
          offset += 8;
        } else if (kind === NULL) {
          value = null;
        } else {
          const length = view.getUint32(offset, true);
          offset += 4;
          const payload = bytes.subarray(offset, offset + length);
          offset += length;
          value = kind === TEXT ? decoder.decode(payload) : payload.slice();
        }
        if (this.#returnArrays) row[index] = value;
        else row[names[index]] = value;
      }
      return row;
    }
  }

  class Session {
    #handle = 0;
    #database;

    constructor(token, database, handle) {
      if (token !== kSession) throw new codes.ERR_ILLEGAL_CONSTRUCTOR();
      this.#database = database;
      this.#handle = handle;
    }

    #requireOpen() {
      if (this.#handle === 0) throw invalidState("session is not open");
    }

    changeset() {
      this.#requireOpen();
      return runOp(() => ops.op_sqlite_session_changeset(this.#handle, false));
    }

    patchset() {
      this.#requireOpen();
      return runOp(() => ops.op_sqlite_session_changeset(this.#handle, true));
    }

    close() {
      this.#requireOpen();
      const handle = this.#handle;
      runOp(() => ops.op_sqlite_session_close(handle));
      this.#handle = 0;
    }
  }

  const sessionOptions = (options) => {
    if (options === null || typeof options !== "object") {
      throw new codes.ERR_INVALID_ARG_TYPE("options", "object", options);
    }
    const table = options.table;
    if (table !== undefined && typeof table !== "string") {
      throw new codes.ERR_INVALID_ARG_TYPE("options.table", "string", table);
    }
    const database = options.db;
    if (database !== undefined && typeof database !== "string") {
      throw new codes.ERR_INVALID_ARG_TYPE("options.db", "string", database);
    }
    return { database: database ?? "main", table: table ?? null };
  };

  const functionOptions = (options) => {
    if (options === null || typeof options !== "object") {
      throw new codes.ERR_INVALID_ARG_TYPE("options", "object", options);
    }
    return {
      useBigIntArguments: booleanOption(options, "useBigIntArguments", false),
      varargs: booleanOption(options, "varargs", false),
      deterministic: booleanOption(options, "deterministic", false),
      directOnly: booleanOption(options, "directOnly", false),
    };
  };

  const registerFunction = (database, name, options, values) => {
    if (typeof name !== "string") {
      throw new codes.ERR_INVALID_ARG_TYPE("name", "string", name);
    }
    const handle = database[kDatabase]();
    runOp(() => ops.op_sqlite_create_function(
      handle,
      name,
      options.varargs ? -1 : values.arguments,
      values.kind,
      options.deterministic,
      options.directOnly,
      options.useBigIntArguments,
      values.callbacks,
    ));
  };

  Database.prototype.function = function (name, options, callback) {
    if (typeof options === "function") {
      callback = options;
      options = {};
    }
    if (callback === undefined) {
      throw new codes.ERR_INVALID_ARG_TYPE("function", "function", callback);
    }
    if (typeof callback !== "function") {
      throw new codes.ERR_INVALID_ARG_TYPE("function", "function", callback);
    }
    const normalized = functionOptions(options ?? {});
    registerFunction(this, name, normalized, {
      kind: 0,
      arguments: callback.length,
      callbacks: [callback],
    });
  };

  Database.prototype.aggregate = function (name, options) {
    if (options === null || typeof options !== "object") {
      throw new codes.ERR_INVALID_ARG_TYPE("options", "object", options);
    }
    const start = options.start;
    if (start === undefined) {
      throw new codes.ERR_INVALID_ARG_TYPE("options.start", "a function or a primitive value", start);
    }
    const step = options.step;
    if (typeof step !== "function") {
      throw new codes.ERR_INVALID_ARG_TYPE("options.step", "function", step);
    }
    for (const key of ["result", "inverse"]) {
      if (options[key] !== undefined && typeof options[key] !== "function") {
        throw new codes.ERR_INVALID_ARG_TYPE("options." + key, "function", options[key]);
      }
    }
    const normalized = functionOptions(options);
    registerFunction(this, name, normalized, {
      kind: 1,
      arguments: Math.max(step.length - 1, 0),
      callbacks: [start, step, options.result ?? null, options.inverse ?? null],
    });
  };

  Database.prototype.createSession = function (options = {}) {
    const normalized = sessionOptions(options);
    const handle = runOp(() =>
      ops.op_sqlite_session_create(
        this[kDatabase](),
        normalized.database,
        normalized.table,
      )
    );
    return new Session(kSession, this, handle);
  };

  Database.prototype.applyChangeset = function (changeset, options = {}) {
    if (!(changeset instanceof Uint8Array)) {
      throw new codes.ERR_INVALID_ARG_TYPE("changeset", "Uint8Array", changeset);
    }
    if (options === null || typeof options !== "object") {
      throw new codes.ERR_INVALID_ARG_TYPE("options", "object", options);
    }
    const filter = options.filter;
    if (filter !== undefined && typeof filter !== "function") {
      throw new codes.ERR_INVALID_ARG_TYPE("options.filter", "function", filter);
    }
    const onConflict = options.onConflict;
    if (onConflict !== undefined && typeof onConflict !== "function") {
      throw new codes.ERR_INVALID_ARG_TYPE("options.onConflict", "function", onConflict);
    }
    return runOp(() =>
      ops.op_sqlite_apply_changeset(
        this[kDatabase](),
        changeset,
        filter ?? null,
        onConflict ?? null,
      )
    );
  };
  const LIMITS = {
    length: 0,
    sqlLength: 1,
    column: 2,
    exprDepth: 3,
    compoundSelect: 4,
    vdbeOp: 5,
    functionArg: 6,
    attach: 7,
    likePatternLength: 8,
    variableNumber: 9,
    triggerDepth: 10,
  };

  const limitObject = (database) => {
    const limits = {};
    for (const [name, code] of Object.entries(LIMITS)) {
      Object.defineProperty(limits, name, {
        enumerable: true,
        configurable: false,
        get() {
          return runOp(() => ops.op_sqlite_limit(database[kDatabase](), code));
        },
        set(value) {
          if (!Number.isInteger(value)) {
            throw new codes.ERR_INVALID_ARG_TYPE("limits." + name, "integer", value);
          }
          runOp(() => ops.op_sqlite_set_limit(database[kDatabase](), code, value));
        },
      });
    }
    return limits;
  };

  Object.defineProperty(Database.prototype, "limits", {
    get() {
      this[kDatabase]();
      return limitObject(this);
    },
  });

  Database.prototype.location = function (database = "main") {
    if (typeof database !== "string") {
      throw new codes.ERR_INVALID_ARG_TYPE("database", "string", database);
    }
    return runOp(() => ops.op_sqlite_location(this[kDatabase](), database));
  };

  Database.prototype.serialize = function (database = "main") {
    if (typeof database !== "string") {
      throw new codes.ERR_INVALID_ARG_TYPE("dbName", "string", database);
    }
    return runOp(() => ops.op_sqlite_serialize(this[kDatabase](), database));
  };

  Database.prototype.deserialize = function (data, database = "main") {
    if (!(data instanceof Uint8Array)) {
      throw new codes.ERR_INVALID_ARG_TYPE("data", "Uint8Array", data);
    }
    if (typeof database !== "string") {
      throw new codes.ERR_INVALID_ARG_TYPE("dbName", "string", database);
    }
    runOp(() => ops.op_sqlite_deserialize(this[kDatabase](), data, database, false));
  };

  Database.prototype.setAuthorizer = function (callback) {
    if (callback !== null && typeof callback !== "function") {
      throw new codes.ERR_INVALID_ARG_TYPE("callback", "function or null", callback);
    }
    runOp(() => ops.op_sqlite_set_authorizer(this[kDatabase](), callback));
  };

  Database.prototype.enableLoadExtension = function (enable) {
    if (typeof enable !== "boolean") {
      throw new codes.ERR_INVALID_ARG_TYPE("enable", "boolean", enable);
    }
    if (enable && !this[kAllowExtension]()) {
      throw invalidState(
        "Cannot enable extension loading because it was disabled at database creation.",
      );
    }
    runOp(() => ops.op_sqlite_enable_load_extension(this[kDatabase](), enable));
  };

  Database.prototype.loadExtension = function (path, entry) {
    if (entry !== undefined && typeof entry !== "string") {
      throw new codes.ERR_INVALID_ARG_TYPE("entry", "string", entry);
    }
    if (!this[kAllowExtension]()) {
      throw invalidState("extension loading is not allowed");
    }
    const extensionPath = databasePath(path);
    runOp(() => ops.op_sqlite_load_extension(this[kDatabase](), extensionPath, entry ?? null));
  };

  Database.prototype.createTagStore = function (maxSize = 1000) {
    if (!Number.isInteger(maxSize) || maxSize <= 0) {
      throw new codes.ERR_INVALID_ARG_TYPE("maxSize", "positive integer", maxSize);
    }
    return new SQLTagStore(this, maxSize);
  };

  class SQLTagStore {
    #database;
    #maxSize;
    #statements = new Map();

    constructor(database, maxSize) {
      this.#database = database;
      this.#maxSize = maxSize;
    }

    get capacity() {
      return this.#maxSize;
    }

    get size() {
      return this.#statements.size;
    }

    get db() {
      return this.#database;
    }

    #prepare(strings, values) {
      if (!Array.isArray(strings)) {
        throw new codes.ERR_INVALID_ARG_TYPE(
          "First argument",
          "an array of strings (template literal)",
          strings,
        );
      }
      let sql = "";
      for (let index = 0; index < strings.length; index += 1) {
        sql += strings[index];
        if (index + 1 < strings.length) sql += "?";
      }
      let statement = this.#statements.get(sql);
      if (statement === undefined) {
        if (this.#statements.size >= this.#maxSize) {
          const [oldest, oldestStatement] = this.#statements.entries().next().value;
          this.#statements.delete(oldest);
          oldestStatement.close();
        }
        statement = this.#database.prepare(sql);
        this.#statements.set(sql, statement);
      }
      return { statement, values };
    }

    get(strings, ...values) {
      const { statement, values: parameters } = this.#prepare(strings, values);
      return statement.get(...parameters);
    }

    all(strings, ...values) {
      const { statement, values: parameters } = this.#prepare(strings, values);
      return statement.all(...parameters);
    }

    run(strings, ...values) {
      const { statement, values: parameters } = this.#prepare(strings, values);
      return statement.run(...parameters);
    }

    iterate(strings, ...values) {
      const { statement, values: parameters } = this.#prepare(strings, values);
      return statement.iterate(...parameters);
    }

    clear() {
      for (const statement of this.#statements.values()) statement.close();
      this.#statements.clear();
    }
  }

  const STATEMENT_COUNTERS = {
    fullscanStep: 1,
    sort: 2,
    autoIndex: 3,
    vmStep: 4,
    reprepare: 5,
    run: 6,
    filterMiss: 7,
    filterHit: 8,
    memused: 9,
  };

  Statement.prototype.stat = function (counter) {
    if (typeof counter !== "string") {
      throw new codes.ERR_INVALID_ARG_TYPE("counter", "string", counter);
    }
    const code = STATEMENT_COUNTERS[counter];
    if (code === undefined) {
      throw invalidArgumentValue('The "counter" argument is not a valid statistic name.');
    }
    return runOp(() => ops.op_sqlite_statement_status(this[kStatement](), code));
  };

  Statement.prototype.resetStats = function (counter) {
    if (counter !== undefined && typeof counter !== "string") {
      throw new codes.ERR_INVALID_ARG_TYPE("counter", "string", counter);
    }
    const code = counter === undefined ? 0 : STATEMENT_COUNTERS[counter];
    if (counter !== undefined && code === undefined) {
      throw invalidArgumentValue('The "counter" argument is not a valid statistic name.');
    }
    runOp(() =>
      ops.op_sqlite_statement_reset_status(this[kStatement](), code ?? 0, counter === undefined)
    );
  };

  Database.prototype.enableDefensive = function (enable) {
    if (typeof enable !== "boolean") {
      throw new codes.ERR_INVALID_ARG_TYPE("enable", "boolean", enable);
    }
    runOp(() => ops.op_sqlite_enable_defensive(this[kDatabase](), enable));
  };

  const quoteIdentifier = (name) => '"' + name.replaceAll('"', '""') + '"';

  Database.prototype.createModule = function (name, options) {
    if (typeof name !== "string") {
      throw new codes.ERR_INVALID_ARG_TYPE("name", "string", name);
    }
    if (options === null || typeof options !== "object") {
      throw new codes.ERR_INVALID_ARG_TYPE("options", "object", options);
    }
    if (!Array.isArray(options.columns)) {
      throw new codes.ERR_INVALID_ARG_TYPE("options.columns", "array", options.columns);
    }
    if (options.columns.length === 0) {
      throw invalidArgumentValue('The "options.columns" array must not be empty.');
    }
    const columns = options.columns.map((column, index) => {
      if (column === null || typeof column !== "object") {
        throw new codes.ERR_INVALID_ARG_TYPE(`options.columns[${index}]`, "object", column);
      }
      if (typeof column.name !== "string") {
        throw new codes.ERR_INVALID_ARG_TYPE(`options.columns[${index}].name`, "string", column.name);
      }
      if (typeof column.type !== "string") {
        throw new codes.ERR_INVALID_ARG_TYPE(`options.columns[${index}].type`, "string", column.type);
      }
      return { name: column.name, type: column.type };
    });
    if (typeof options.rows !== "function") {
      throw new codes.ERR_INVALID_ARG_TYPE("options.rows", "function", options.rows);
    }
    const declaration = "CREATE TABLE x(" +
      columns.map((column) => `${quoteIdentifier(column.name)} ${column.type}`).join(", ") + ")";
    const names = columns.map((column) => column.name);
    const rows = () => {
      const returned = options.rows();
      let entries;
      if (returned !== null && typeof returned === "object" &&
          typeof returned.next === "function") {
        entries = Array.from(returned);
      } else if (returned !== null && typeof returned === "object" &&
          typeof returned[Symbol.iterator] === "function") {
        entries = Array.from(returned);
      } else if (Array.isArray(returned)) {
        entries = returned;
      } else {
        throw new codes.ERR_INVALID_ARG_TYPE("options.rows result", "iterable", returned);
      }
      return entries.map((row) => {
        if (Array.isArray(row)) return row;
        if (row === null || typeof row !== "object") {
          throw new codes.ERR_INVALID_ARG_TYPE("options.rows entry", "array or object", row);
        }
        return names.map((column) => row[column]);
      });
    };
    runOp(() => ops.op_sqlite_create_module(this[kDatabase](), name, declaration, rows));
  };

  const backup = async (sourceDb, path, options = {}) => {
    if (sourceDb === null || typeof sourceDb !== "object" ||
        typeof sourceDb[kDatabase] !== "function") {
      throw new codes.ERR_INVALID_ARG_TYPE("sourceDb", "DatabaseSync", sourceDb);
    }
    if (options === null || typeof options !== "object") {
      throw new codes.ERR_INVALID_ARG_TYPE("options", "object", options);
    }
    const rate = integerOption(options, "rate", 100);
    if (rate === 0) {
      throw new codes.ERR_OUT_OF_RANGE("options.rate", "> 0", rate);
    }
    const progress = options.progress;
    if (progress !== undefined && typeof progress !== "function") {
      throw new codes.ERR_INVALID_ARG_TYPE("options.progress", "function", progress);
    }
    runOp(() =>
      ops.op_sqlite_backup(
        sourceDb[kDatabase](),
        databasePath(path),
        "main",
        "main",
        rate,
        progress ?? null,
      )
    );
  };
  module.exports = {
    Database,
    DatabaseSync: Database,
    Statement,
    StatementSync: Statement,
    Session,
    constants,
    backup,
  };
})();
