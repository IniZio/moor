import * as v from 'valibot';

import { SignedOut } from './api';

export type Unsaved = { ahead: number; dirty: number };

/** Thrown when the controller said no, such as a Box that holds work that is not pushed. */
export class Refused extends Error {
	constructor(
		why: string,
		readonly unsaved?: Unsaved,
	) {
		super(why);
	}
}

/** Thrown when an interrupt went through but the message after it did not. */
export class InterruptedOnly extends Error {}

const UnsavedSchema = v.object({ refusal: v.literal('unsaved'), ahead: v.number(), dirty: v.number() });
const FailureSchema = v.object({
	error: v.string(),
	interrupted: v.optional(v.boolean()),
	refusal: v.optional(v.looseObject({ refusal: v.string() })),
});
const SentSchema = v.object({
	delivery: v.optional(v.picklist(['delivered', 'queued', 'in_doubt']), 'delivered'),
	cause: v.nullish(v.string()),
});

/** How the agent took a message: now, or from the Box's inbox once it can. */
export type Sent = v.InferOutput<typeof SentSchema>;

/** What a message says about the prompt the page shows open, so the message answers that prompt. */
export type Over = { ask: string; pick: number };

type Body = Record<string, string | number | boolean | undefined>;

async function post<T extends v.GenericSchema>(path: string, csrf: string, body: Body, schema: T): Promise<v.InferOutput<T>> {
	const res = await fetch(`/account/api/box/${path}`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json', 'X-Moor-Csrf': csrf },
		body: JSON.stringify(body),
	});
	if (res.status === 401) throw new SignedOut();
	const reply: unknown = await res.json().catch(() => null);
	if (res.ok) return v.parse(schema, reply);
	const failure = v.safeParse(FailureSchema, reply);
	const why = failure.success ? failure.output.error : `The gate answered ${res.status}.`;
	if (failure.success && failure.output.interrupted) throw new InterruptedOnly(why);
	const unsaved = v.safeParse(UnsavedSchema, failure.success ? failure.output.refusal : null);
	throw res.status === 403 ? new Refused(why, unsaved.success ? unsaved.output : undefined) : new Error(why);
}

/** Sends the agent a message after its current tool call, or stops its turn first. */
export async function boxSteer(csrf: string, box: string, text: string, interrupt: boolean, over?: Over): Promise<Sent> {
	return post('steer', csrf, { box, text, interrupt, ask: over?.ask, pick: over?.pick }, SentSchema);
}

/** Ends the agent and puts the Box to sleep. */
export async function boxStop(csrf: string, box: string): Promise<void> {
	await post('stop', csrf, { box }, v.unknown());
}

/** Removes the Box; `force` closes it past work that is not pushed. */
export async function boxClose(csrf: string, box: string, force: boolean): Promise<void> {
	await post('close', csrf, { box, force }, v.unknown());
}
