export interface AccountDeletionContext<Socket> {
  getWebSockets(): Iterable<Socket>;
  storage: { deleteAll(): Promise<void>; get?(key: string): Promise<unknown>; put?(key: string, value: unknown): Promise<void> };
}

export async function deleteAccountState<Socket>(
  ctx: AccountDeletionContext<Socket>,
  close: (socket: Socket, code: number, reason: string) => void,
): Promise<void> {
  for (const socket of ctx.getWebSockets()) close(socket, 4001, "signed out");
  await ctx.storage.deleteAll();
}

export const SIGNING_KEY = "self:signing-key";

/** A self-hosted deployment keeps its token-signing key through a wipe: Worker isolates hold it for
 * their lifetime, and with every device row gone no old token is accepted anyway. */
export async function deleteSelfState<Socket>(
  ctx: AccountDeletionContext<Socket>,
  close: (socket: Socket, code: number, reason: string) => void,
): Promise<void> {
  const key = await ctx.storage.get?.(SIGNING_KEY);
  await deleteAccountState(ctx, close);
  if (key !== undefined) await ctx.storage.put?.(SIGNING_KEY, key);
}
