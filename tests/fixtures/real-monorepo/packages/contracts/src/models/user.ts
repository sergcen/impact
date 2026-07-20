export interface User { id: string }
export const userVersion = 1;
export default function createUser(id: string): User { return { id }; }
