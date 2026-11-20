import { useEffect, useState } from 'preact/hooks';

import { failure, toError } from './api';

export function useLoad<T>(load: () => Promise<T>, key: string) {
	const [value, setValue] = useState<T | null>(null);
	const [error, setError] = useState('');
	const reload = async () => {
		setError('');
		try {
			setValue(await load());
		} catch (err) {
			setError(failure(toError(err)));
		}
	};
	useEffect(() => {
		setValue(null);
		void reload();
	}, [key]);
	return { value, setValue, error, reload };
}
