const farmingContractName = 'dev-1649371698726-40356935804134'
const nftContractName = 'dev-1649259045266-73594760682734'
const ftContractName = 'dev-1649119497564-55770252445071'
const ownerAccountName = 'pruebaprueba.testnet'

module.exports = function getConfig(network = 'mainnet') {
	let config = {
		networkId: "testnet",
		nodeUrl: "https://rpc.testnet.near.org",
		walletUrl: "https://wallet.testnet.near.org",
		helperUrl: "https://helper.testnet.near.org",
        farmingContractName: farmingContractName,
        nftContractName: nftContractName,
        ftContractName: ftContractName,
        ownerAccountName: ownerAccountName
	}

	switch (network) {
	case 'testnet':
		config = {
			explorerUrl: "https://explorer.testnet.near.org",
			...config,
			GAS: "200000000000000",
			gas: "200000000000000",
			gas_max: "300000000000000",
			DEFAULT_NEW_ACCOUNT_AMOUNT: "2",
			DEFAULT_NEW_CONTRACT_AMOUNT: "5",
			GUESTS_ACCOUNT_SECRET: "7UVfzoKZL4WZGF98C3Ue7tmmA6QamHCiB1Wd5pkxVPAc7j6jf3HXz5Y9cR93Y68BfGDtMLQ9Q29Njw5ZtzGhPxv",
        }
    }

	return config
}