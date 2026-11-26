import { type AgentContract, contractRead } from './api';
import { useLoad } from './useLoad';
import { Failed } from './WorkflowBits';

/** Where the contract page lives. */
export const CONTRACT_HREF = '#/settings/contract';

/** The contract version a run's agent read, linked to the contract page. */
export function ContractVersion({ version }: { version: number }) {
	return <a href={CONTRACT_HREF}>contract v{version}</a>;
}

/** The running contract as moor hands it to agents: its version, then the file, raw. */
export function ContractView({ contract }: { contract: AgentContract }) {
	return (
		<>
			<h2>Agent contract</h2>
			<p class="muted">
				Version {contract.version}, sha256 <code>{contract.sha256.slice(0, 12)}</code>. Every agent moor launches reads it first.
			</p>
			<pre class="contract-raw" aria-label="The contract file">{contract.text}</pre>
		</>
	);
}

export function ContractPage() {
	const { value, error, reload } = useLoad(contractRead, 'contract');
	if (error) return <Failed error={error} retry={reload} />;
	return value ? <ContractView contract={value} /> : <p class="muted" aria-busy="true">Loading the contract…</p>;
}
