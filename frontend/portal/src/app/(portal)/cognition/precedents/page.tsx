"use client";

import { Freshness, KeyValue } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { formatCount } from "@/lib/format";
import {
  isRecord,
  isRecordList,
  precedentFields,
  precedentValue,
  recordListColumns,
  usePrecedents,
  type Precedent,
} from "@/lib/hooks/useCognition";
import { CognitionHeader, Muted } from "../_shared";

/**
 * The precedents the REASON stage recorded beside each hypothesis, read-only.
 *
 * `GET /cognition/precedents` answers one record per hypothesis the loop
 * convened on — the kernel's own `HypothesisPrecedent`, serialised as the
 * kernel serialises it, oldest first — and the fields are the kernel's to
 * choose. This page renders every field of every record as a definition
 * list, `similarity`, `outcome` and `age` first where they are present and
 * the rest in the order they came, and drops nothing: a field this console
 * did not anticipate is still on the screen, because a page that showed only
 * the fields it knew would hide the day the memory started answering
 * something new. A field that is itself a record (`digest`) is a nested
 * list; a field that is a list of records (`nearest[]`, best first) is a
 * table whose columns are every key any row carries, in the same order.
 *
 * Nothing here recalls, stores, evicts or re-ranks. The nearest neighbours,
 * their similarity and their agreement are the platform's finding, recorded
 * on the hypothesis for replay and moving its confidence by nothing; this
 * page is the record of that and cannot be its input.
 */
export default function PrecedentsPage() {
  const precedents = usePrecedents();

  return (
    <div className="flex flex-col gap-3 p-3">
      <CognitionHeader
        title="Precedents"
        reads="GET /cognition/precedents"
        meta={<Freshness resource={precedents} name="precedents" />}
      />

      <Panel>
        <PanelHead title="Recalled episodes" />
        <PanelBody>
          <ResourceView resource={precedents} loadingRows={3}>
            {(data) => (
              <>
                <KpiRow>
                  <Kpi
                    label="Precedents recalled"
                    value={<span data-testid="precedent-count">{formatCount(data.precedents.length)}</span>}
                    note="GET /cognition/precedents: precedents"
                  />
                </KpiRow>
                {data.precedents.length === 0 ? (
                  <div className="mt-3" data-testid="precedents-empty">
                    <EmptyBlock headline="The memory has recalled no precedent.">
                      <p>
                        No episode has been recalled in this process. A precedent is recalled from
                        strictly after its known-at instant against a hypothesis the loop is reasoning
                        about, so until one has been there is nothing to show — this is an observed empty
                        recall, not an unread one.
                      </p>
                    </EmptyBlock>
                  </div>
                ) : (
                  <div className="mt-3 flex flex-col gap-3">
                    {data.precedents.map((precedent, index) => (
                      <PrecedentCard key={index} precedent={precedent} index={index} />
                    ))}
                  </div>
                )}
                <p className="mt-2">
                  <Muted>
                    Every field of every precedent is shown as it came, similarity, outcome and age
                    first where the memory answered them. This page does not re-rank or filter them.
                  </Muted>
                </p>
              </>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>
    </div>
  );
}

function PrecedentCard({ precedent, index }: { precedent: Precedent; index: number }) {
  const fields = precedentFields(precedent);
  return (
    <section
      className="flex flex-col gap-1 border border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)] px-3 py-2"
      data-testid="precedent"
      aria-label={`precedent ${index + 1}`}
    >
      <span className="eyebrow">precedent {index + 1}</span>
      {fields.length === 0 ? (
        <Muted>the memory answered a record with no field</Muted>
      ) : (
        <FieldList record={precedent} fields={fields} testId="precedent-field" />
      )}
    </section>
  );
}

/** Every field of a record, as it came, each drawn by the arm its shape asks for. */
function FieldList({
  record,
  fields,
  testId,
}: {
  record: Readonly<Record<string, unknown>>;
  fields: readonly string[];
  testId: string;
}) {
  return (
    <dl>
      {fields.map((field) => {
        const value = record[field];
        if (isRecordList(value)) {
          return (
            <div key={field} className="py-1" data-testid={testId} data-field={field} data-shape="table">
              <span className="text-[11px] text-[color:var(--color-ink-dim)]">
                {field} · {formatCount(value.length)} row(s), in the platform&rsquo;s order
              </span>
              <RecordTable rows={value} label={field} />
            </div>
          );
        }
        if (isRecord(value)) {
          const nested = precedentFields(value);
          return (
            <div key={field} className="py-1" data-testid={testId} data-field={field} data-shape="record">
              <span className="text-[11px] text-[color:var(--color-ink-dim)]">{field}</span>
              <div className="ml-4">
                {nested.length === 0 ? (
                  <Muted>an empty record</Muted>
                ) : (
                  <FieldList record={value} fields={nested} testId={`${testId}-nested`} />
                )}
              </div>
            </div>
          );
        }
        if (Array.isArray(value) && value.length === 0) {
          return (
            <div key={field} data-testid={testId} data-field={field} data-shape="empty-list">
              <KeyValue label={field} mono={false}>
                an empty list
              </KeyValue>
            </div>
          );
        }
        return (
          <div key={field} data-testid={testId} data-field={field} data-shape="scalar">
            <KeyValue label={field}>{precedentValue(value)}</KeyValue>
          </div>
        );
      })}
    </dl>
  );
}

/** A list of records as a table: one column per key any row carries, one row per record. */
function RecordTable({ rows, label }: { rows: readonly Readonly<Record<string, unknown>>[]; label: string }) {
  const columns = recordListColumns(rows);
  return (
    <TableWell maxHeight="320px" label={`${label}, in the platform's order`}>
      <table className="dt" data-testid="precedent-table">
        <thead>
          <tr>
            {columns.map((column) => (
              <th key={column} scope="col">
                {column}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, index) => (
            <tr key={index} data-testid="precedent-table-row">
              {columns.map((column) => (
                <td key={column} className="num" data-column={column}>
                  {precedentValue(row[column])}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </TableWell>
  );
}
