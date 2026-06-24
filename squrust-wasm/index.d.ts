/* TypeScript definitions for the Squrust wasm package. */

export class SqurustDb {
  /** Open a transient in-memory database. */
  static openMemory(): Promise<SqurustDb>;

  /** Run a SELECT, resolving to one object per row keyed by column name. */
  query(sql: string, params?: unknown[]): Promise<Record<string, unknown>[]>;

  /** Run a DDL/DML statement, resolving to the number of rows affected. */
  execute(sql: string, params?: unknown[]): Promise<number>;

  /** Release the database. */
  close(): Promise<void>;
}

export default function init(input?: RequestInfo | URL | Response | BufferSource | WebAssembly.Module): Promise<unknown>;
