// ── Mock Data Store ──────────────────────────────────────────────
'use strict';

const generateHistory = (base, days, volatility = 0.05) => {
  const prices = [];
  let p = base * (0.7 + Math.random() * 0.3);
  for (let i = days; i >= 0; i--) {
    p = p * (1 + (Math.random() - 0.48) * volatility);
    prices.push(+p.toFixed(2));
  }
  prices[prices.length - 1] = base;
  return prices;
};

const AppData = {
  user: {
    id: 'USR001',
    name: 'Alex Mercer',
    email: 'alex.mercer@email.com',
    phone: '+1 (555) 012-3456',
    avatar: null,
    initials: 'AM',
    kycStatus: 'verified',
    memberSince: 'Jan 2024',
    tier: 'Pro',
    referralCode: 'ALEX2024',
    referralCount: 12,
    referralEarnings: 145.50,
    twoFAEnabled: true,
    biometricEnabled: true,
    portfolioVisible: true,
    language: 'English',
    currency: 'USD',
    theme: 'dark',
    notificationsEnabled: true,
  },

  portfolio: {
    totalValue: 47823.56,
    totalInvested: 38200.00,
    totalPnL: 9623.56,
    totalPnLPct: 25.19,
    dayChange: 1234.78,
    dayChangePct: 2.65,
    performanceHistory7d: generateHistory(47823, 7, 0.025),
    performanceHistory30d: generateHistory(47823, 30, 0.035),
    performanceHistory90d: generateHistory(47823, 90, 0.04),
    performanceHistory1y: generateHistory(47823, 365, 0.05),
  },

  holdings: [
    { coin: 'BTC', amount: 0.4521, valueUSD: 19847.23, pnl: 4215.00, pnlPct: 27.0, weight: 41.5 },
    { coin: 'ETH', amount: 4.821, valueUSD: 12312.45, pnl: 2811.00, pnlPct: 29.6, weight: 25.7 },
    { coin: 'SOL', amount: 68.5, valueUSD: 6748.90, pnl: 1250.00, pnlPct: 22.7, weight: 14.1 },
    { coin: 'BNB', amount: 12.4, valueUSD: 3651.20, pnl: 421.00, pnlPct: 13.0, weight: 7.6 },
    { coin: 'AVAX', amount: 82.1, valueUSD: 2621.10, pnl: 512.00, pnlPct: 24.3, weight: 5.5 },
    { coin: 'LINK', amount: 190.0, valueUSD: 1482.60, pnl: 190.00, pnlPct: 14.7, weight: 3.1 },
    { coin: 'ADA', amount: 2800, valueUSD: 1160.08, pnl: 210.00, pnlPct: 22.1, weight: 2.5 },
  ],

  coins: [
    { id: 'btc', symbol: 'BTC', name: 'Bitcoin', price: 43900.24, change24h: 2.34, changePct7d: 8.12, marketCap: 861200000000, volume24h: 28400000000, high24h: 44250.00, low24h: 42800.00, ath: 69000.00, color: '#F7931A', history7d: generateHistory(43900, 7), history30d: generateHistory(43900, 30), description: 'Bitcoin is a decentralized digital currency, without a central bank or single administrator.', rank: 1, supply: 19600000, maxSupply: 21000000 },
    { id: 'eth', symbol: 'ETH', name: 'Ethereum', price: 2554.87, change24h: 3.21, changePct7d: 11.4, marketCap: 306000000000, volume24h: 14200000000, high24h: 2610.00, low24h: 2480.00, ath: 4878.26, color: '#627EEA', history7d: generateHistory(2554, 7), history30d: generateHistory(2554, 30), description: 'Ethereum is a decentralized open-source blockchain system that features smart contract functionality.', rank: 2, supply: 120000000, maxSupply: null },
    { id: 'bnb', symbol: 'BNB', name: 'BNB', price: 294.62, change24h: 1.05, changePct7d: 4.2, marketCap: 44100000000, volume24h: 1120000000, high24h: 298.00, low24h: 289.00, ath: 686.31, color: '#F3BA2F', history7d: generateHistory(294, 7), history30d: generateHistory(294, 30), description: 'BNB is the cryptocurrency of the Binance exchange and powers the BNB Chain ecosystem.', rank: 4, supply: 153432897, maxSupply: 200000000 },
    { id: 'sol', symbol: 'SOL', name: 'Solana', price: 98.57, change24h: 4.87, changePct7d: 14.3, marketCap: 42600000000, volume24h: 2840000000, high24h: 101.20, low24h: 93.45, ath: 259.96, color: '#9945FF', history7d: generateHistory(98, 7), history30d: generateHistory(98, 30), description: 'Solana is a high-performance blockchain supporting builders around the world creating crypto apps.', rank: 5, supply: 432500000, maxSupply: null },
    { id: 'ada', symbol: 'ADA', name: 'Cardano', price: 0.4143, change24h: -1.23, changePct7d: 2.1, marketCap: 14600000000, volume24h: 441000000, high24h: 0.4290, low24h: 0.4020, ath: 3.09, color: '#0D1E2D', history7d: generateHistory(0.414, 7), history30d: generateHistory(0.414, 30), description: 'Cardano is a proof-of-stake blockchain platform with a research-first driven approach.', rank: 10, supply: 35200000000, maxSupply: 45000000000 },
    { id: 'avax', symbol: 'AVAX', name: 'Avalanche', price: 31.94, change24h: 5.62, changePct7d: 16.8, marketCap: 12900000000, volume24h: 810000000, high24h: 33.10, low24h: 29.80, ath: 146.22, color: '#E84142', history7d: generateHistory(31, 7), history30d: generateHistory(31, 30), description: 'Avalanche is the fastest smart contracts platform in the blockchain industry.', rank: 11, supply: 403700000, maxSupply: 720000000 },
    { id: 'dot', symbol: 'DOT', name: 'Polkadot', price: 8.21, change24h: 2.14, changePct7d: 6.5, marketCap: 10400000000, volume24h: 320000000, high24h: 8.45, low24h: 7.98, ath: 54.98, color: '#E6007A', history7d: generateHistory(8.2, 7), history30d: generateHistory(8.2, 30), description: 'Polkadot enables cross-blockchain transfers of any type of data or asset.', rank: 13, supply: 1270000000, maxSupply: null },
    { id: 'matic', symbol: 'MATIC', name: 'Polygon', price: 0.8934, change24h: -0.87, changePct7d: 3.2, marketCap: 8310000000, volume24h: 498000000, high24h: 0.9120, low24h: 0.8740, ath: 2.92, color: '#8247E5', history7d: generateHistory(0.89, 7), history30d: generateHistory(0.89, 30), description: 'Polygon is a protocol and framework for building and connecting Ethereum-compatible blockchain networks.', rank: 14, supply: 9300000000, maxSupply: 10000000000 },
    { id: 'link', symbol: 'LINK', name: 'Chainlink', price: 7.8, change24h: 1.95, changePct7d: 7.4, marketCap: 4560000000, volume24h: 410000000, high24h: 8.02, low24h: 7.61, ath: 52.70, color: '#2A5ADA', history7d: generateHistory(7.8, 7), history30d: generateHistory(7.8, 30), description: 'Chainlink decentralizes oracle networks to provide smart contracts with trusted data.', rank: 15, supply: 587000000, maxSupply: 1000000000 },
    { id: 'uni', symbol: 'UNI', name: 'Uniswap', price: 7.12, change24h: 3.44, changePct7d: 9.8, marketCap: 4270000000, volume24h: 185000000, high24h: 7.35, low24h: 6.87, ath: 44.97, color: '#FF007A', history7d: generateHistory(7.1, 7), history30d: generateHistory(7.1, 30), description: 'Uniswap is a leading decentralized crypto exchange that runs on the Ethereum blockchain.', rank: 18, supply: 600000000, maxSupply: 1000000000 },
    { id: 'atom', symbol: 'ATOM', name: 'Cosmos', price: 10.54, change24h: -2.11, changePct7d: 1.2, marketCap: 3090000000, volume24h: 174000000, high24h: 10.92, low24h: 10.21, ath: 44.70, color: '#2E3148', history7d: generateHistory(10.5, 7), history30d: generateHistory(10.5, 30), description: 'Cosmos is a decentralized network of independent, scalable, and interoperable blockchains.', rank: 22, supply: 293000000, maxSupply: null },
    { id: 'near', symbol: 'NEAR', name: 'NEAR Protocol', price: 3.42, change24h: 6.21, changePct7d: 18.4, marketCap: 3400000000, volume24h: 320000000, high24h: 3.58, low24h: 3.18, ath: 20.44, color: '#00C08B', history7d: generateHistory(3.4, 7), history30d: generateHistory(3.4, 30), description: 'NEAR Protocol is a developer-friendly and climate-neutral blockchain protocol.', rank: 23, supply: 994000000, maxSupply: null },
    { id: 'algo', symbol: 'ALGO', name: 'Algorand', price: 0.1852, change24h: 1.43, changePct7d: 5.6, marketCap: 1510000000, volume24h: 88000000, high24h: 0.1920, low24h: 0.1790, ath: 3.28, color: '#00B4D8', history7d: generateHistory(0.185, 7), history30d: generateHistory(0.185, 30), description: 'Algorand is a blockchain network aiming to be secure, scalable and decentralized.', rank: 35, supply: 8200000000, maxSupply: 10000000000 },
    { id: 'xlm', symbol: 'XLM', name: 'Stellar', price: 0.1241, change24h: -0.54, changePct7d: 2.8, marketCap: 2920000000, volume24h: 148000000, high24h: 0.1280, low24h: 0.1195, ath: 0.8752, color: '#14B6E7', history7d: generateHistory(0.124, 7), history30d: generateHistory(0.124, 30), description: 'Stellar is an open network for storing and moving money.', rank: 30, supply: 28700000000, maxSupply: 50000000000 },
    { id: 'arb', symbol: 'ARB', name: 'Arbitrum', price: 1.23, change24h: 4.12, changePct7d: 12.1, marketCap: 3900000000, volume24h: 421000000, high24h: 1.29, low24h: 1.16, ath: 2.39, color: '#28A0F0', history7d: generateHistory(1.23, 7), history30d: generateHistory(1.23, 30), description: 'Arbitrum is a Layer-2 optimistic rollup solution for Ethereum.', rank: 20, supply: 3200000000, maxSupply: 10000000000 },
    { id: 'op', symbol: 'OP', name: 'Optimism', price: 2.54, change24h: 5.32, changePct7d: 15.6, marketCap: 3050000000, volume24h: 310000000, high24h: 2.64, low24h: 2.38, ath: 4.84, color: '#FF0420', history7d: generateHistory(2.54, 7), history30d: generateHistory(2.54, 30), description: 'Optimism is a Layer-2 scaling solution for Ethereum using optimistic rollup technology.', rank: 25, supply: 1200000000, maxSupply: 4294967296 },
    { id: 'sui', symbol: 'SUI', name: 'Sui', price: 1.87, change24h: 7.14, changePct7d: 21.3, marketCap: 2010000000, volume24h: 540000000, high24h: 1.95, low24h: 1.72, ath: 2.18, color: '#4DA2FF', history7d: generateHistory(1.87, 7), history30d: generateHistory(1.87, 30), description: 'Sui is a next-generation smart contract platform with high throughput, low latency.', rank: 28, supply: 1075000000, maxSupply: 10000000000 },
    { id: 'apt', symbol: 'APT', name: 'Aptos', price: 9.42, change24h: 3.87, changePct7d: 10.2, marketCap: 3870000000, volume24h: 287000000, high24h: 9.75, low24h: 9.01, ath: 19.90, color: '#00B5D8', history7d: generateHistory(9.4, 7), history30d: generateHistory(9.4, 30), description: 'Aptos is a new layer 1 blockchain built by former Meta Diem team members.', rank: 26, supply: 412000000, maxSupply: null },
    { id: 'ftm', symbol: 'FTM', name: 'Fantom', price: 0.5421, change24h: -1.87, changePct7d: 4.1, marketCap: 1510000000, volume24h: 124000000, high24h: 0.5580, low24h: 0.5230, ath: 3.46, color: '#1969FF', history7d: generateHistory(0.542, 7), history30d: generateHistory(0.542, 30), description: 'Fantom is a directed acyclic graph (DAG) smart contract platform providing DeFi services.', rank: 40, supply: 2800000000, maxSupply: 3175000000 },
    { id: 'ltc', symbol: 'LTC', name: 'Litecoin', price: 74.21, change24h: 0.92, changePct7d: 3.4, marketCap: 5540000000, volume24h: 390000000, high24h: 75.80, low24h: 73.10, ath: 410.26, color: '#BFBBBB', history7d: generateHistory(74, 7), history30d: generateHistory(74, 30), description: 'Litecoin is a peer-to-peer cryptocurrency and open-source software project.', rank: 16, supply: 74600000, maxSupply: 84000000 },
  ],

  transactions: [
    { id: 'TX001', type: 'buy', coin: 'BTC', amount: 0.05, price: 42100, total: 2105, date: '2024-03-10 14:32', status: 'completed', fee: 2.11 },
    { id: 'TX002', type: 'sell', coin: 'ETH', amount: 1.2, price: 2480, total: 2976, date: '2024-03-09 11:14', status: 'completed', fee: 2.98 },
    { id: 'TX003', type: 'deposit', coin: 'USDT', amount: 5000, price: 1, total: 5000, date: '2024-03-08 09:45', status: 'completed', fee: 0 },
    { id: 'TX004', type: 'buy', coin: 'SOL', amount: 20, price: 92.50, total: 1850, date: '2024-03-07 16:55', status: 'completed', fee: 1.85 },
    { id: 'TX005', type: 'swap', coin: 'BNB', amount: 5, price: 290, total: 1450, date: '2024-03-06 12:30', status: 'completed', fee: 1.45 },
    { id: 'TX006', type: 'withdraw', coin: 'USDT', amount: 2000, price: 1, total: 2000, date: '2024-03-05 08:20', status: 'completed', fee: 1.00 },
    { id: 'TX007', type: 'buy', coin: 'AVAX', amount: 30, price: 27.80, total: 834, date: '2024-03-04 18:42', status: 'completed', fee: 0.83 },
    { id: 'TX008', type: 'stake', coin: 'SOL', amount: 10, price: 95.20, total: 952, date: '2024-03-03 10:15', status: 'completed', fee: 0 },
    { id: 'TX009', type: 'buy', coin: 'LINK', amount: 50, price: 7.40, total: 370, date: '2024-03-02 14:00', status: 'completed', fee: 0.37 },
    { id: 'TX010', type: 'sell', coin: 'ADA', amount: 1000, price: 0.42, total: 420, date: '2024-03-01 09:30', status: 'completed', fee: 0.42 },
    { id: 'TX011', type: 'buy', coin: 'ETH', amount: 0.5, price: 2510, total: 1255, date: '2024-02-28 15:22', status: 'pending', fee: 1.26 },
  ],

  notifications: [
    { id: 'N001', type: 'price_alert', title: 'Bitcoin Price Alert', message: 'BTC crossed $44,000 target', time: '2m ago', read: false, icon: 'trending-up' },
    { id: 'N002', type: 'trade', title: 'Trade Executed', message: 'Bought 0.05 BTC at $43,850', time: '1h ago', read: false, icon: 'check-circle' },
    { id: 'N003', type: 'ai', title: 'AI Recommendation', message: 'New portfolio rebalancing suggestion available', time: '3h ago', read: false, icon: 'cpu' },
    { id: 'N004', type: 'staking', title: 'Rewards Earned', message: 'Earned 0.42 SOL staking rewards', time: '5h ago', read: true, icon: 'award' },
    { id: 'N005', type: 'market', title: 'Market Surge', message: 'SOL is up 8.2% in the last hour', time: '6h ago', read: true, icon: 'zap' },
    { id: 'N006', type: 'security', title: 'New Login', message: 'New sign-in from Chrome on Windows', time: '1d ago', read: true, icon: 'shield' },
    { id: 'N007', type: 'kyc', title: 'KYC Verified', message: 'Your identity has been verified successfully', time: '2d ago', read: true, icon: 'user-check' },
    { id: 'N008', type: 'deposit', title: 'Deposit Confirmed', message: '$5,000 USDT deposited successfully', time: '3d ago', read: true, icon: 'download' },
  ],

  aiRecommendations: [
    { id: 'AI001', type: 'buy', coin: 'ETH', confidence: 87, reason: 'Strong accumulation signal detected. ETH/USD showing bullish divergence on 4H RSI. Network activity up 23% this week.', action: 'Buy at $2,480-$2,520 range', timeframe: '1-2 weeks', risk: 'Medium', targetPrice: 2850, stopLoss: 2350 },
    { id: 'AI002', type: 'hold', coin: 'BTC', confidence: 92, reason: 'Bitcoin is consolidating in a healthy range. On-chain metrics show strong holder conviction. Institutional inflows positive.', action: 'Hold current position', timeframe: '2-4 weeks', risk: 'Low', targetPrice: 52000, stopLoss: 40000 },
    { id: 'AI003', type: 'buy', coin: 'SOL', confidence: 79, reason: 'Solana ecosystem showing explosive growth. DeFi TVL up 45% month-over-month. Technical breakout imminent.', action: 'Accumulate on dips to $90-95', timeframe: '2-3 weeks', risk: 'Medium-High', targetPrice: 130, stopLoss: 82 },
    { id: 'AI004', type: 'sell', coin: 'ADA', confidence: 71, reason: 'Cardano facing network activity headwinds. Development activity declining vs competitors. Rebalance allocation.', action: 'Reduce position by 30%', timeframe: '1 week', risk: 'Low', targetPrice: 0.38, stopLoss: 0.45 },
    { id: 'AI005', type: 'buy', coin: 'ARB', confidence: 83, reason: 'Arbitrum L2 adoption accelerating. Transaction volume at all-time high. Ecosystem growth outpacing competitors.', action: 'Buy $1.10-$1.20 range', timeframe: '3-4 weeks', risk: 'Medium', targetPrice: 1.80, stopLoss: 0.95 },
  ],

  staking: [
    { id: 'S001', coin: 'SOL', amount: 48.5, apy: 6.8, rewards: 1.24, rewardsUSD: 122.22, lockPeriod: 'Flexible', startDate: '2024-02-01', status: 'active', totalEarned: 3.82 },
    { id: 'S002', coin: 'ETH', amount: 1.0, apy: 4.2, rewards: 0.0042, rewardsUSD: 10.73, lockPeriod: '30 days', startDate: '2024-01-15', status: 'active', totalEarned: 0.014 },
    { id: 'S003', coin: 'AVAX', amount: 50.0, apy: 9.5, rewards: 0.42, rewardsUSD: 13.41, lockPeriod: '60 days', startDate: '2024-01-10', status: 'active', totalEarned: 1.98 },
    { id: 'S004', coin: 'MATIC', amount: 1000, apy: 5.2, rewards: 4.68, rewardsUSD: 4.18, lockPeriod: 'Flexible', startDate: '2024-02-20', status: 'active', totalEarned: 14.5 },
  ],

  stakingOptions: [
    { coin: 'ETH', apy: 4.2, minAmount: 0.1, lockPeriod: '30 days', description: 'Liquid staking via Lido', risk: 'Low' },
    { coin: 'SOL', apy: 6.8, minAmount: 1, lockPeriod: 'Flexible', description: 'Native Solana staking', risk: 'Low' },
    { coin: 'AVAX', apy: 9.5, minAmount: 10, lockPeriod: '60 days', description: 'AVAX validator staking', risk: 'Medium' },
    { coin: 'MATIC', apy: 5.2, minAmount: 10, lockPeriod: 'Flexible', description: 'Polygon delegated staking', risk: 'Low' },
    { coin: 'DOT', apy: 14.2, minAmount: 1, lockPeriod: '28 days', description: 'Polkadot parachain staking', risk: 'Medium' },
    { coin: 'ATOM', apy: 18.5, minAmount: 0.1, lockPeriod: '21 days', description: 'Cosmos validator delegation', risk: 'Medium' },
    { coin: 'NEAR', apy: 11.0, minAmount: 1, lockPeriod: 'Flexible', description: 'NEAR staking pool', risk: 'Low-Medium' },
    { coin: 'ADA', apy: 4.6, minAmount: 5, lockPeriod: 'Flexible', description: 'Cardano delegation', risk: 'Low' },
  ],

  watchlist: [
    { coin: 'BTC', alert: null, addedAt: '2024-01-01' },
    { coin: 'ETH', alert: { above: 3000, below: 2200 }, addedAt: '2024-01-02' },
    { coin: 'SOL', alert: { above: 120, below: 80 }, addedAt: '2024-02-01' },
    { coin: 'AVAX', alert: null, addedAt: '2024-02-15' },
    { coin: 'ARB', alert: { above: 1.50, below: 1.00 }, addedAt: '2024-03-01' },
  ],

  priceAlerts: [
    { id: 'PA001', coin: 'ETH', condition: 'above', price: 3000, created: '2024-03-01', active: true },
    { id: 'PA002', coin: 'SOL', condition: 'above', price: 120, created: '2024-03-05', active: true },
    { id: 'PA003', coin: 'BTC', condition: 'below', price: 40000, created: '2024-02-20', active: true },
    { id: 'PA004', coin: 'ARB', condition: 'above', price: 1.50, created: '2024-03-08', active: false },
  ],

  orders: [
    { id: 'O001', type: 'limit', side: 'buy', coin: 'BTC', amount: 0.02, limitPrice: 41500, total: 830, status: 'open', created: '2024-03-10' },
    { id: 'O002', type: 'stop', side: 'sell', coin: 'ETH', amount: 1.0, stopPrice: 2400, total: 2400, status: 'open', created: '2024-03-09' },
    { id: 'O003', type: 'limit', side: 'buy', coin: 'SOL', amount: 10, limitPrice: 88, total: 880, status: 'filled', created: '2024-03-08' },
  ],

  news: [
    { id: 'NW001', coin: 'BTC', title: 'Bitcoin ETF Inflows Hit $1B Daily Record', source: 'CoinDesk', time: '2h ago', sentiment: 'positive', url: '#' },
    { id: 'NW002', coin: 'ETH', title: 'Ethereum Layer 2 TVL Surpasses $20 Billion', source: 'The Block', time: '4h ago', sentiment: 'positive', url: '#' },
    { id: 'NW003', coin: 'SOL', title: 'Solana Processes 50M Daily Transactions', source: 'Decrypt', time: '6h ago', sentiment: 'positive', url: '#' },
    { id: 'NW004', coin: 'BTC', title: 'Fed Rate Decision Could Impact Crypto Markets', source: 'Bloomberg', time: '8h ago', sentiment: 'neutral', url: '#' },
    { id: 'NW005', coin: 'AVAX', title: 'Avalanche Subnet Activity Hits New ATH', source: 'CryptoSlate', time: '10h ago', sentiment: 'positive', url: '#' },
    { id: 'NW006', coin: 'ARB', title: 'Arbitrum Governance Proposal Passes with 89% Approval', source: 'DeFi Llama', time: '12h ago', sentiment: 'positive', url: '#' },
  ],

  aiChat: [
    { role: 'ai', content: "Hello! I'm your AI trading assistant. How can I help you today? I can analyze market trends, provide portfolio recommendations, and help with risk management." },
    { role: 'user', content: "What's your outlook for Bitcoin this week?" },
    { role: 'ai', content: "Based on current on-chain metrics and technical analysis, Bitcoin shows strong bullish momentum. Key observations:\n\n• RSI at 62 — healthy, not overbought\n• Whale accumulation up 15% this week\n• Resistance at $45,200; support at $42,800\n\nMy model gives 74% probability of testing $45,000 within 7 days." },
    { role: 'user', content: 'Should I buy more ETH now?' },
    { role: 'ai', content: "ETH presents a good opportunity based on current conditions:\n\n✅ Strong: Network activity, staking yields, L2 ecosystem growth\n⚠️ Watch: ETH/BTC ratio consolidating\n\nSuggestion: Dollar-cost average 20-30% of your target position now at $2,480-2,520. Stop loss: $2,280." },
  ],

  faq: [
    { q: 'How do I verify my identity (KYC)?', a: "Go to Profile → Security → Identity Verification. You'll need a government-issued ID and a selfie. Verification typically takes 2-10 minutes." },
    { q: 'What are the trading fees?', a: 'Maker fee: 0.1%, Taker fee: 0.15%. Pro tier members get 25% discount. No deposit fees. Withdrawal fees vary by network.' },
    { q: 'How does AI trading advisor work?', a: 'Our AI analyzes on-chain data, technical indicators, market sentiment, and macroeconomic factors to generate personalized investment recommendations.' },
    { q: 'What is staking?', a: 'Staking allows you to earn passive income by locking your crypto assets to support network validation. APY varies by asset from 4% to 18%+.' },
    { q: 'How do I set price alerts?', a: "Go to any coin's detail page or the Watchlist section. Tap the bell icon to set alerts for price levels you want to monitor." },
    { q: 'Is my crypto safe?', a: 'We use military-grade encryption, cold storage for 95% of assets, multi-signature wallets, and regular security audits by top blockchain security firms.' },
    { q: 'How do I withdraw funds?', a: 'Go to Wallet → Withdraw. Select the asset, enter amount and destination address. Withdrawals are processed within 1-30 minutes depending on network congestion.' },
    { q: 'What is the referral program?', a: 'Invite friends using your unique referral code. Earn 30% of their trading fees for 12 months. No limit on referrals. Rewards paid in USDT.' },
  ],

  getCoin(symbol) {
    return this.coins.find(c => c.symbol === symbol.toUpperCase());
  },
  getCoinById(id) {
    return this.coins.find(c => c.id === id);
  },
  formatPrice(price) {
    if (price >= 1000) return '$' + price.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
    if (price >= 1) return '$' + price.toFixed(2);
    if (price >= 0.01) return '$' + price.toFixed(4);
    return '$' + price.toFixed(6);
  },
  formatLargeNum(n) {
    if (n >= 1e12) return '$' + (n / 1e12).toFixed(2) + 'T';
    if (n >= 1e9) return '$' + (n / 1e9).toFixed(2) + 'B';
    if (n >= 1e6) return '$' + (n / 1e6).toFixed(2) + 'M';
    return '$' + n.toFixed(0);
  },
  formatChange(pct) {
    const sign = pct >= 0 ? '+' : '';
    return sign + pct.toFixed(2) + '%';
  },
  getChangeClass(pct) {
    return pct >= 0 ? 'text-success' : 'text-error';
  },

  assetNews: [
    { id: 1, title: 'Bitcoin Surges Past $45,000 as Institutional Inflows Accelerate', source: 'CoinDesk', time: '2 hours ago', sentiment: 'positive', url: '#' },
    { id: 2, title: 'Network Hashrate Hits New All-Time High', source: 'Decrypt', time: '5 hours ago', sentiment: 'positive', url: '#' },
    { id: 3, title: 'Analysts Monitor Potential Sell Pressure from Miner Wallets', source: 'Bloomberg Crypto', time: '12 hours ago', sentiment: 'negative', url: '#' },
    { id: 4, title: 'Weekly Market Report: Bitcoin Consolidates Above Key Support', source: 'CoinTelegraph', time: '1 day ago', sentiment: 'neutral', url: '#' },
    { id: 5, title: 'Major Update Coming to Core Bitcoin Protocol Next Month', source: 'The Block', time: '2 days ago', sentiment: 'positive', url: '#' },
    { id: 6, title: 'Exchange Outflows Point to Long-Term Holding Trend', source: 'CryptoSlate', time: '3 days ago', sentiment: 'neutral', url: '#' }
  ]
};

window.AppData = AppData;
