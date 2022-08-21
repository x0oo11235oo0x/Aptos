// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

import {
  Center, IconButton, SimpleGrid, useColorMode,
} from '@chakra-ui/react';
import { IoMdImage } from 'react-icons/io';
import { RiCopperCoinFill } from 'react-icons/ri';
import React from 'react';
import { useLocation } from 'react-router-dom';
import { SettingsIcon } from '@chakra-ui/icons';
import ChakraLink from './ChakraLink';

const secondaryHeaderBgColor = {
  dark: 'gray.700',
  light: 'gray.200',
};

const secondaryIconColor = {
  dark: 'white',
  light: 'gray.800',
};

export default function WalletFooter() {
  const { colorMode } = useColorMode();
  const { pathname } = useLocation();

  return (
    <Center
      maxW="100%"
      width="100%"
      py={2}
      bgColor={secondaryHeaderBgColor[colorMode]}
    >
      <SimpleGrid width="100%" gap={4} columns={3}>
        <Center width="100%">
          <ChakraLink to="/wallet">
            <IconButton
              color={(pathname.includes('/wallet')) ? 'blue.400' : secondaryIconColor[colorMode]}
              variant="unstyled"
              size="md"
              aria-label="Wallet"
              fontSize="xl"
              icon={<RiCopperCoinFill />}
              display="flex"
            />
          </ChakraLink>
        </Center>
        <Center width="100%">
          <ChakraLink to="/gallery">
            <IconButton
              color={(pathname.includes('/gallery')) ? 'blue.400' : secondaryIconColor[colorMode]}
              variant="unstyled"
              size="md"
              aria-label="Gallery"
              icon={<IoMdImage />}
              fontSize="xl"
              display="flex"
            />
          </ChakraLink>
        </Center>
        <Center width="100%">
          <ChakraLink to="/settings">
            <IconButton
              color={(pathname.includes('/settings')) ? 'blue.400' : secondaryIconColor[colorMode]}
              variant="unstyled"
              size="md"
              aria-label="Account"
              icon={<SettingsIcon />}
              fontSize="xl"
              display="flex"
            />
          </ChakraLink>
        </Center>
      </SimpleGrid>
    </Center>
  );
}
