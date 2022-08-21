// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

import {
  HStack,
  VStack,
} from '@chakra-ui/react';
import React from 'react';
import withSimulatedExtensionContainer from 'core/components/WithSimulatedExtensionContainer';
import WalletLayout from 'core/layouts/WalletLayout';
import WalletAccountBalance from 'core/components/WalletAccountBalance';
import TransferDrawer from 'core/components/TransferDrawer';
import Faucet from 'core/components/Faucet';

function Wallet() {
  return (
    <WalletLayout>
      <VStack width="100%" paddingTop={8}>
        <WalletAccountBalance />
        <HStack spacing={4}>
          <Faucet />
          <TransferDrawer />
        </HStack>
      </VStack>
    </WalletLayout>
  );
}

export default withSimulatedExtensionContainer(Wallet);
