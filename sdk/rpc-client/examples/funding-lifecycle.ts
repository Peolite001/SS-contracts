// End-to-end InvoiceEscrow funding lifecycle: create -> invest -> release ->
// settle, with the refund branch for invoices that miss their threshold.
//
// Run against a deployed escrow + payment token (+ payment distributor wired to
// the escrow by the admin) with:
//   ESCROW_CONTRACT_ID=C... TOKEN_ID=C... ADMIN_SECRET=S... SELLER_SECRET=S... \
//   INVESTOR_SECRET=S... npx tsx examples/funding-lifecycle.ts
//
// Every `submit` call below needs a signature from the keypair passed to it.
import { Address, Keypair, nativeToScVal, rpc, xdr } from "@stellar/stellar-sdk";
import { contractErrorsForCode } from "@stellarstate/ss-contracts-sdk";
import { InvoiceEscrowRpcClient } from "@stellarstate/ss-contracts-rpc";

const env = (name: string): string => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
};

const client = new InvoiceEscrowRpcClient({
  contractId: env("ESCROW_CONTRACT_ID"),
  rpcUrl: process.env.STELLAR_RPC_URL ?? "https://soroban-testnet.stellar.org",
});
const token = env("TOKEN_ID");
const admin = Keypair.fromSecret(env("ADMIN_SECRET"));
const seller = Keypair.fromSecret(env("SELLER_SECRET"));
const investor = Keypair.fromSecret(env("INVESTOR_SECRET"));

const addr = (id: string): xdr.ScVal => new Address(id).toScVal();
const i128 = (n: bigint): xdr.ScVal => nativeToScVal(n, { type: "i128" });
const u32 = (n: number): xdr.ScVal => nativeToScVal(n, { type: "u32" });
const invoiceId = nativeToScVal(Buffer.alloc(32, 7), { type: "bytes" });

/** Submit, then map a contract failure to its typed InvoiceEscrow error name. */
async function call(method: string, args: xdr.ScVal[], signer: Keypair): Promise<void> {
  try {
    const result = await client.submit(method, args, signer);
    if (result.status !== rpc.Api.GetTransactionStatus.SUCCESS) {
      throw new Error(`${method} finished with status ${result.status}`);
    }
    console.log(`${method}: ${result.status}`);
  } catch (err) {
    // Simulation surfaces contract errors as "Error(Contract, #<code>)".
    const code = /Error\(Contract, #(\d+)\)/.exec(String(err))?.[1];
    if (code) {
      const names = contractErrorsForCode("invoice-escrow", Number(code));
      throw new Error(`${method} rejected: ${names.join(" | ")} (#${code})`);
    }
    throw err;
  }
}

async function main(): Promise<void> {
  // 1. Seller lists the invoice (signature: seller).
  await call(
    "create_invoice",
    [
      invoiceId,
      addr(seller.publicKey()),
      i128(1_000_000n), // funding_target
      i128(100_000n), // min_funding_threshold
      i128(1_000_000n), // max_funding_threshold
      u32(500_000), // deadline_ledger
      i128(1_000n), // min_investment
      nativeToScVal(null), // per_investor_cap: None
      addr(token),
    ],
    seller,
  );

  // 2. Investor funds it in the payment token (signature: investor).
  await call("invest", [invoiceId, addr(investor.publicKey()), i128(250_000n)], investor);

  try {
    // 3. Admin releases the raised funds to the issuer (signature: admin).
    await call("release_to_issuer", [invoiceId], admin);
    // 4. Admin settles; the distributor pays investors (signature: admin).
    await call("settle_invoice", [invoiceId, i128(1_050_000n)], admin);
  } catch (err) {
    // Refund branch, e.g. FundingThresholdNotReached (signature: investor).
    console.error(String(err));
    await call("refund", [invoiceId, addr(investor.publicKey())], investor);
  }

  console.log("invoice:", await client.query("get_invoice", { invoice_id: Buffer.alloc(32, 7) }));
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
