// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

import { AddIcon } from '@chakra-ui/icons';
import {
  Button,
  FormControl,
  FormLabel,
  Input,
  Modal,
  ModalBody,
  ModalCloseButton,
  ModalContent,
  ModalFooter,
  ModalHeader,
  ModalOverlay,
  Text,
  useColorMode,
  useDisclosure,
  VStack,
} from '@chakra-ui/react';
import { AptosClient, RequestError, TokenClient } from 'aptos';
import { useMutation, useQueryClient } from 'react-query';
import React from 'react';
import { SubmitHandler, useForm } from 'react-hook-form';
import useWalletState from 'core/hooks/useWalletState';
import { secondaryTextColor } from 'pages/Login';
import { NODE_URL } from 'core/constants';
import { AptosAccountState } from 'core/types';
import { AptosNetwork } from 'core/utils/network';

// eslint-disable-next-line global-require
window.Buffer = window.Buffer || require('buffer').Buffer;

export const defaultRequestErrorAttributes = {
  config: {},
  headers: {},
  status: 400,
  statusText: 'Move abort',
};

export interface RaiseForErrorProps {
  vmStatus: string
}

const raiseForError = ({
  vmStatus,
}: RaiseForErrorProps) => {
  if (vmStatus.includes('Move abort')) {
    throw new RequestError(vmStatus, {
      data: {
        message: vmStatus,
      },
      ...defaultRequestErrorAttributes,
    });
  }
};

interface CreateTokenAndCollectionProps {
  account: AptosAccountState;
  collectionName?: string;
  description?: string;
  name?: string;
  nodeUrl?: AptosNetwork;
  supply: number;
  uri?: string;
}

const createTokenAndCollection = async ({
  account,
  collectionName,
  description,
  name,
  nodeUrl = NODE_URL,
  supply,
  uri,
}: CreateTokenAndCollectionProps): Promise<void> => {
  if (!account || !(collectionName && description && uri && name)) {
    return;
  }
  const aptosClient = new AptosClient(nodeUrl);
  const tokenClient = new TokenClient(aptosClient);

  const collectionTxnHash = await tokenClient.createCollection(
    account,
    collectionName,
    description,
    uri,
  );

  // Move abort errors do not throw so we need to check them manually
  const collectionTxn: any = await aptosClient.getTransaction(collectionTxnHash);
  let vmStatus: string = collectionTxn.vm_status;
  raiseForError({ vmStatus });

  const tokenTxnHash = await tokenClient.createToken(
    account,
    collectionName,
    name,
    description,
    supply,
    uri,
  );
  const tokenTxn: any = await aptosClient.getTransaction(tokenTxnHash);
  vmStatus = tokenTxn.vm_status;
  raiseForError({ vmStatus });
};

export default function CreateNFTModal() {
  const { colorMode } = useColorMode();
  const { isOpen, onClose, onOpen } = useDisclosure();
  const { handleSubmit, register, watch } = useForm();
  const { aptosAccount, aptosNetwork } = useWalletState();
  const queryClient = useQueryClient();

  const collectionName: string | undefined = watch('collectionName');
  const tokenName: string | undefined = watch('tokenName');
  const description: string | undefined = watch('description');
  const supply = Number(watch('supply') || 1);
  const uri: string | undefined = watch('uri');

  const {
    error,
    isError,
    isLoading,
    mutateAsync: createTokenAndCollectionOnClick,
  } = useMutation<void, RequestError>(() => (
    createTokenAndCollection({
      account: aptosAccount,
      collectionName,
      description,
      name: tokenName,
      nodeUrl: aptosNetwork,
      supply,
      uri,
    })
  ));

  const errorMessage = error?.response?.data?.message;

  const onSubmit: SubmitHandler<Record<string, any>> = async (_data, event) => {
    event?.preventDefault();
    await createTokenAndCollectionOnClick();
    await queryClient.refetchQueries(['gallery-items']);
    onClose();
  };

  return (
    <>
      <Button size="xs" onClick={onOpen} leftIcon={<AddIcon fontSize="xs" />}>
        New
      </Button>
      <Modal isOpen={isOpen} onClose={onClose}>
        <ModalOverlay />
        <ModalContent>
          <form onSubmit={handleSubmit(onSubmit)}>
            <ModalHeader>Create an NFT</ModalHeader>
            <ModalCloseButton />
            <ModalBody>
              <VStack>
                <FormControl isRequired>
                  <FormLabel fontWeight={400} color={secondaryTextColor[colorMode]}>
                    Collection name
                  </FormLabel>
                  <Input
                    {...register('collectionName')}
                    variant="filled"
                    required
                    maxLength={100}
                  />
                </FormControl>
                <FormControl isRequired>
                  <FormLabel fontWeight={400} color={secondaryTextColor[colorMode]}>
                    Token name
                  </FormLabel>
                  <Input
                    {...register('tokenName')}
                    variant="filled"
                    required
                    maxLength={100}
                  />
                </FormControl>
                <FormControl isRequired>
                  <FormLabel fontWeight={400} color={secondaryTextColor[colorMode]}>
                    Description
                  </FormLabel>
                  <Input
                    {...register('description')}
                    variant="filled"
                    required
                    maxLength={3000}
                    placeholder="A description of your collection"
                  />
                </FormControl>
                <FormControl isRequired>
                  <FormLabel fontWeight={400} color={secondaryTextColor[colorMode]}>
                    Supply
                  </FormLabel>
                  <Input
                    {...register('supply')}
                    variant="filled"
                    type="number"
                    min={1}
                    required
                    defaultValue={1}
                    max={1e9}
                  />
                </FormControl>
                <FormControl isRequired>
                  <FormLabel fontWeight={400} color={secondaryTextColor[colorMode]}>
                    Uri
                  </FormLabel>
                  <Input
                    {...register('uri')}
                    variant="filled"
                    required
                    maxLength={300}
                    placeholder="Arweave, IPFS, or S3 uri"
                  />
                </FormControl>
                {
                  (isError)
                    ? (
                      <Text color="red.400">
                        {errorMessage}
                      </Text>
                    )
                    : undefined
                }
              </VStack>
            </ModalBody>
            <ModalFooter>
              <Button isLoading={isLoading} colorScheme="blue" mr={3} type="submit">
                Submit
              </Button>
              <Button variant="ghost" onClick={onClose}>Close</Button>
            </ModalFooter>
          </form>
        </ModalContent>
      </Modal>
    </>
  );
}
