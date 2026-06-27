"""
Backtest Tom Nash's DCA system on TSLA and PLTR, then compare to options strategies.

Strategy from the video (Tom Nash, May 2026):
- Monthly DCA: $1000 per stock per month
- If stock is down 20% from 12-month high -> 2x DCA
- If stock is up >20% in past 30 days -> 0.5x DCA
- If position is up 50% from avg cost -> trim 10% to cash
- If position is up 100% from avg cost -> trim 20% to cash
- Trims accumulate in cash pool and get redeployed

Options overlays:
1. Covered calls: sell ATM call against 25% of position monthly
2. Cash-secured puts: deploy DCA via CSP instead of market buy
"""

import yfinance as yf
import pandas as pd
import numpy as np
from datetime import datetime, timedelta
import warnings
warnings.filterwarnings('ignore')

TICKERS = ["TSLA", "PLTR"]
START = "2021-01-01"
END = "2026-05-08"
MONTHLY = 1000  # monthly allocation per stock


def get_data(ticker):
    df = yf.download(ticker, start=START, end=END, auto_adjust=True, progress=False)
    if isinstance(df.columns, pd.MultiIndex):
        df = df.xs(ticker, axis=1, level=1)
    if isinstance(df, pd.DataFrame) and "Close" in df.columns:
        df = df[["Close"]]
    elif isinstance(df, pd.DataFrame):
        df = df.iloc[:, [3]]
        df.columns = ["Close"]
    if isinstance(df, pd.Series):
        df = df.to_frame("Close")
    df["Close"] = df["Close"].squeeze().astype(float)
    return df[["Close"]].copy()


def run_dca(df):
    """
    Tom Nash DCA system:
    - Each month: add $MONTHLY to cash
    - Buy decision based on price vs 12mo high and 30d return
    - Trim rules based on position return vs avg cost
    """
    df = df.copy()
    df["month"] = df.index.to_period("M")

    shares = 0.0
    cash = 0.0
    total_invested = 0.0
    total_trimmed = 0.0
    avg_cost = 0.0
    trades = []

    for period, group in df.groupby("month"):
        month_start = group.index[0]
        month_end = group.index[-1]
        price = float(group["Close"].iloc[-1])

        # Add monthly income
        cash += MONTHLY

        # Calculate indicators
        high_12m = float(df["Close"].rolling(252).max().loc[month_end])
        if pd.isna(high_12m) or high_12m == 0:
            high_12m = price

        price_30d = float(df["Close"].asof(month_end - timedelta(days=30)))
        if pd.isna(price_30d) or price_30d == 0:
            price_30d = price
        ret_30d = (price / price_30d) - 1

        # Decide DCA amount
        dca = MONTHLY
        reason = "normal"
        if price <= high_12m * 0.80:
            dca = MONTHLY * 2
            reason = "2x_down_20pct"
        if ret_30d > 0.20:
            dca *= 0.5
            reason = "half_speed_up_20pct_30d"

        # Execute buy
        buy_shares = dca / price if price > 0 else 0
        cash -= dca
        total_invested += dca
        old_cost = avg_cost * shares
        shares += buy_shares
        avg_cost = (old_cost + dca) / shares if shares > 0 else 0
        trades.append({
            "date": month_end, "type": "buy", "price": price,
            "amount": dca, "shares": buy_shares, "reason": reason
        })

        # Trim rules: check position return
        if shares > 0 and avg_cost > 0:
            pos_return = (price / avg_cost) - 1

            if pos_return >= 1.0:
                trim_shares = shares * 0.20
                trim_val = trim_shares * price
                shares -= trim_shares
                cash += trim_val
                total_trimmed += trim_val
                trades.append({
                    "date": month_end, "type": "trim_100pct",
                    "price": price, "amount": trim_val,
                    "shares_sold": trim_shares
                })

            if pos_return >= 0.50:
                trim_shares = shares * 0.10
                trim_val = trim_shares * price
                shares -= trim_shares
                cash += trim_val
                total_trimmed += trim_val
                trades.append({
                    "date": month_end, "type": "trim_50pct",
                    "price": price, "amount": trim_val,
                    "shares_sold": trim_shares
                })

    # Final value
    final_price = float(df["Close"].iloc[-1])
    final_value = shares * final_price + cash
    total_return = (final_value / total_invested - 1) * 100
    years = (df.index[-1] - df.index[0]).days / 365.25
    cagr = ((final_value / total_invested) ** (1 / years) - 1) * 100 if years > 0 else 0

    # Max drawdown (approximate: monthly)
    monthly_values = []
    for period, group in df.groupby("month"):
        price = float(group["Close"].iloc[-1])
        monthly_values.append(price * shares + cash)  # rough — shares/cash change monthly
    if monthly_values:
        peak = np.maximum.accumulate(monthly_values)
        dd = (np.array(monthly_values) / peak - 1) * 100
        max_dd = float(np.min(dd))
    else:
        max_dd = 0

    return {
        "total_invested": round(total_invested, 2),
        "final_value": round(final_value, 2),
        "total_return_pct": round(total_return, 2),
        "cagr_pct": round(cagr, 2),
        "max_drawdown_pct": round(max_dd, 2),
        "final_shares": round(shares, 4),
        "avg_cost": round(avg_cost, 2),
        "cash_on_hand": round(cash, 2),
        "trades": trades,
        "num_buys": sum(1 for t in trades if t["type"] == "buy"),
        "num_trims": sum(1 for t in trades if t["type"].startswith("trim")),
        "total_trimmed": round(total_trimmed, 2),
        "double_downs": sum(1 for t in trades if t.get("reason") == "2x_down_20pct"),
        "half_speeds": sum(1 for t in trades if t.get("reason") == "half_speed_up_20pct_30d"),
    }


def run_buy_hold(df):
    """Simple monthly DCA into stock, no trims."""
    df = df.copy()
    df["month"] = df.index.to_period("M")

    shares = 0.0
    total_invested = 0.0

    for period, group in df.groupby("month"):
        price = float(group["Close"].iloc[-1])
        buy = MONTHLY / price
        shares += buy
        total_invested += MONTHLY

    final_price = float(df["Close"].iloc[-1])
    final_value = shares * final_price
    total_return = (final_value / total_invested - 1) * 100
    years = (df.index[-1] - df.index[0]).days / 365.25
    cagr = ((final_value / total_invested) ** (1 / years) - 1) * 100 if years > 0 else 0

    # Max drawdown (monthly approximation)
    monthly_vals = []
    for period, group in df.groupby("month"):
        price = float(group["Close"].iloc[-1])
        monthly_vals.append(price * shares)
    if monthly_vals:
        peak = np.maximum.accumulate(monthly_vals)
        dd = (np.array(monthly_vals) / peak - 1) * 100
        max_dd = float(np.min(dd))
    else:
        max_dd = 0

    return {
        "total_invested": round(total_invested, 2),
        "final_value": round(final_value, 2),
        "total_return_pct": round(total_return, 2),
        "cagr_pct": round(cagr, 2),
        "max_drawdown_pct": round(max_dd, 2),
        "final_shares": round(shares, 4),
    }


def run_options(df):
    """
    Options-aware version:
    - Sell cash-secured puts ATM each month instead of market-buy
    - If assigned, hold shares and sell covered calls on 25% of position
    - Track premium income separate from price return
    """
    df = df.copy()
    df["month"] = df.index.to_period("M")

    shares = 0.0
    cash = 0.0
    total_invested = 0.0
    premium_collected = 0.0
    avg_cost = 0.0
    trades = []

    CSP_PREMIUM = 0.02  # 2% of notional per month for ATM puts
    CC_PREMIUM = 0.015  # 1.5% per month for ATM calls
    CC_COVERAGE = 0.25  # sell calls on 25% of position

    for period, group in df.groupby("month"):
        month_end = group.index[-1]
        price = float(group["Close"].iloc[-1])

        # Add monthly income
        cash += MONTHLY

        high_12m = float(df["Close"].rolling(252).max().loc[month_end])
        if pd.isna(high_12m) or high_12m == 0:
            high_12m = price
        price_30d = float(df["Close"].asof(month_end - timedelta(days=30)))
        if pd.isna(price_30d) or price_30d == 0:
            price_30d = price
        ret_30d = (price / price_30d) - 1

        # Decide deployment amount
        dca = MONTHLY
        if price <= high_12m * 0.80:
            dca = MONTHLY * 2
        if ret_30d > 0.20:
            dca *= 0.5

        # Sell cash-secured put
        csp_premium = dca * CSP_PREMIUM
        premium_collected += csp_premium
        cash += csp_premium
        cash -= dca  # set aside for assignment

        # Simplified assignment: ~50% chance of being ITM at expiry
        strike = round(price)
        assigned = price <= strike * 0.98

        if assigned:
            buy_shares = dca / strike
            old_cost = avg_cost * shares
            shares += buy_shares
            total_invested += dca
            avg_cost = (old_cost + dca) / shares if shares > 0 else 0
            trades.append({
                "date": month_end, "type": "csp_assigned",
                "strike": strike, "premium": csp_premium, "shares": buy_shares
            })
        else:
            cash += dca  # put expired, cash unlocked
            trades.append({
                "date": month_end, "type": "csp_expired",
                "strike": strike, "premium": csp_premium
            })

        # Sell covered calls on 25% of position
        if shares > 0:
            cc_shares = shares * CC_COVERAGE
            cc_notional = cc_shares * price
            cc_premium = cc_notional * CC_PREMIUM
            premium_collected += cc_premium
            cash += cc_premium

            called_away = price >= strike * 1.03  # simplified: ITM call gets exercised
            if called_away:
                proceeds = cc_shares * price
                shares -= cc_shares
                cash += proceeds
                trades.append({
                    "date": month_end, "type": "cc_assigned",
                    "strike": price, "premium": cc_premium,
                    "shares_sold": cc_shares
                })
            else:
                trades.append({
                    "date": month_end, "type": "cc_expired",
                    "strike": price, "premium": cc_premium
                })

        # Trim rules (same as DCA)
        if shares > 0 and avg_cost > 0:
            pos_return = (price / avg_cost) - 1
            if pos_return >= 1.0:
                trim_shares = shares * 0.20
                trim_val = trim_shares * price
                shares -= trim_shares
                cash += trim_val
                trades.append({
                    "date": month_end, "type": "trim_100pct",
                    "price": price, "amount": trim_val,
                    "shares_sold": trim_shares
                })
            if pos_return >= 0.50:
                trim_shares = shares * 0.10
                trim_val = trim_shares * price
                shares -= trim_shares
                cash += trim_val
                trades.append({
                    "date": month_end, "type": "trim_50pct",
                    "price": price, "amount": trim_val,
                    "shares_sold": trim_shares
                })

    final_price = float(df["Close"].iloc[-1])
    final_value = shares * final_price + cash
    total_return = (final_value / total_invested - 1) * 100 if total_invested > 0 else 0
    years = (df.index[-1] - df.index[0]).days / 365.25
    cagr = ((final_value / total_invested) ** (1 / years) - 1) * 100 if total_invested > 0 and years > 0 else 0

    return {
        "total_invested": round(total_invested, 2),
        "final_value": round(final_value, 2),
        "total_return_pct": round(total_return, 2),
        "cagr_pct": round(cagr, 2),
        "final_shares": round(shares, 4),
        "avg_cost": round(avg_cost, 2),
        "cash_on_hand": round(cash, 2),
        "premium_collected": round(premium_collected, 2),
        "trades": trades,
    }


# ---------------------------------------------------------------------------
# Run
# ---------------------------------------------------------------------------

print("=" * 72)
print("Tom Nash DCA System Backtest — TSLA & PLTR")
print(f"Period: {START} to {END}")
print(f"Monthly allocation: ${MONTHLY:,} per stock (added to cash pool each month)")
print("=" * 72)

for ticker in TICKERS:
    df = get_data(ticker)

    dca = run_dca(df)
    bh = run_buy_hold(df)
    opt = run_options(df)

    first_price = float(df["Close"].iloc[0])
    last_price = float(df["Close"].iloc[-1])
    price_chg = (last_price / first_price - 1) * 100

    print(f"\n{'─' * 72}")
    print(f"  {ticker}")
    print(f"  Price: ${first_price:.0f} -> ${last_price:.0f} ({price_chg:+.2f}%)")
    print(f"{'─' * 72}")
    print(f"  {'Metric':<30} {'DCA System':>13} {'Buy-Hold':>13} {'Options':>13}")
    print(f"  {'─'*30} {'─'*13} {'─'*13} {'─'*13}")

    for key, label, fmt in [
        ("total_invested", "Total Invested", "$"),
        ("final_value", "Final Value", "$"),
        ("total_return_pct", "Total Return", "%"),
        ("cagr_pct", "CAGR", "%"),
        ("max_drawdown_pct", "Max Drawdown", "%"),
    ]:
        vals = [dca.get(key, 0), bh.get(key, 0), opt.get(key, 0)]
        if fmt == "$":
            strs = [f"${v:,.0f}" for v in vals]
        else:
            strs = [f"{v:.2f}%" for v in vals]
        print(f"  {label:<30} {strs[0]:>13} {strs[1]:>13} {strs[2]:>13}")

    prem = opt.get("premium_collected", 0)
    ch_dca = f"${dca['cash_on_hand']:,.0f}"
    ch_opt = f"${opt['cash_on_hand']:,.0f}"
    ac_dca = f"${dca['avg_cost']:.2f}"
    ac_opt = f"${opt['avg_cost']:.2f}"
    print(f"  {'Premium Collected':<30} {'N/A':>13} {'N/A':>13} {f'${prem:,.0f}':>13}")
    print(f"  {'Cash on Hand':<30} {ch_dca:>13} {'N/A':>13} {ch_opt:>13}")
    print(f"  {'Avg Cost Basis':<30} {ac_dca:>13} {'N/A':>13} {ac_opt:>13}")

    print(f"\n  DCA System: {dca['num_buys']} buys, {dca['num_trims']} trims")
    print(f"  Double-downs: {dca['double_downs']} | Half-speeds: {dca['half_speeds']} | Total trimmed: ${dca['total_trimmed']:,.0f}")

    csp_a = sum(1 for t in opt["trades"] if t["type"] == "csp_assigned")
    csp_e = sum(1 for t in opt["trades"] if t["type"] == "csp_expired")
    cc_a = sum(1 for t in opt["trades"] if t["type"] == "cc_assigned")
    print(f"  Options: {csp_a} CSP assigned, {csp_e} expired, {cc_a} calls assigned")

print(f"\n{'=' * 72}")
print("Notes:")
print("- Options overlay uses simplified premium: ~2%/mo CSP, ~1.5%/mo covered calls")
print("- CSP assignment: simulated as price <= 0.98x strike at month-end")
print("- CC assignment: simulated as price >= 1.03x strike")
print("- Past performance does not guarantee future results")
