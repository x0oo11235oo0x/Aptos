// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

use crate::{
    data_notification::{
        DataClientRequest, DataPayload, EpochEndingLedgerInfosRequest, PendingClientResponse,
    },
    data_stream::{DataStream, DataStreamListener},
    streaming_client::{
        GetAllEpochEndingLedgerInfosRequest, GetAllStatesRequest, GetAllTransactionsRequest,
        NotificationFeedback, StreamRequest,
    },
    tests::utils::{
        create_data_client_response, create_ledger_info, create_random_u64,
        create_transaction_list_with_proof, get_data_notification, initialize_logger,
        MockAptosDataClient, NoopResponseCallback, MAX_ADVERTISED_EPOCH_END, MAX_ADVERTISED_STATES,
        MAX_ADVERTISED_TRANSACTION_OUTPUT, MAX_NOTIFICATION_TIMEOUT_SECS, MIN_ADVERTISED_EPOCH_END,
        MIN_ADVERTISED_STATES, MIN_ADVERTISED_TRANSACTION_OUTPUT,
    },
};
use aptos_config::config::DataStreamingServiceConfig;
use aptos_data_client::{
    AdvertisedData, GlobalDataSummary, OptimalChunkSizes, Response, ResponseContext,
    ResponsePayload,
};
use aptos_id_generator::U64IdGenerator;
use aptos_infallible::Mutex;
use aptos_types::proof::SparseMerkleRangeProof;
use aptos_types::state_store::state_value::StateValueChunkWithProof;
use aptos_types::{ledger_info::LedgerInfoWithSignatures, transaction::Version};
use claim::{assert_err, assert_ge, assert_matches, assert_none, assert_ok};
use futures::{FutureExt, StreamExt};
use std::{sync::Arc, time::Duration};
use storage_service_types::responses::CompleteDataRange;
use tokio::time::timeout;

#[tokio::test]
async fn test_stream_blocked() {
    // Create a state value stream
    let streaming_service_config = DataStreamingServiceConfig::default();
    let (mut data_stream, mut stream_listener) =
        create_state_value_stream(streaming_service_config, MIN_ADVERTISED_STATES);

    // Initialize the data stream
    let global_data_summary = create_global_data_summary(100);
    data_stream
        .initialize_data_requests(global_data_summary.clone())
        .unwrap();

    let mut number_of_refetches = 0;
    loop {
        // Clear the pending queue and insert a response with an invalid type
        let client_request =
            DataClientRequest::EpochEndingLedgerInfos(EpochEndingLedgerInfosRequest {
                start_epoch: 0,
                end_epoch: 0,
            });
        let context = ResponseContext {
            id: 0,
            response_callback: Box::new(NoopResponseCallback),
        };
        let pending_response = PendingClientResponse {
            client_request: client_request.clone(),
            client_response: Some(Ok(Response {
                context,
                payload: ResponsePayload::NumberOfStates(10),
            })),
        };
        insert_response_into_pending_queue(&mut data_stream, pending_response);

        // Process the data responses and force a data re-fetch
        data_stream
            .process_data_responses(global_data_summary.clone())
            .unwrap();

        // If we're sent a data notification, verify it's an end of stream notification!
        if let Ok(data_notification) = timeout(
            Duration::from_secs(MAX_NOTIFICATION_TIMEOUT_SECS),
            stream_listener.select_next_some(),
        )
        .await
        {
            match data_notification.data_payload {
                DataPayload::EndOfStream => {
                    assert_eq!(
                        number_of_refetches,
                        streaming_service_config.max_request_retry
                    );

                    // Provide incorrect feedback for the notification
                    assert_err!(data_stream.handle_notification_feedback(
                        &data_notification.notification_id,
                        &NotificationFeedback::PayloadTypeIsIncorrect
                    ));

                    // Provide valid feedback for the notification
                    assert_ok!(data_stream.handle_notification_feedback(
                        &data_notification.notification_id,
                        &NotificationFeedback::EndOfStream,
                    ));
                    return;
                }
                data_payload => panic!("Unexpected payload type: {:?}", data_payload),
            }
        }
        number_of_refetches += 1;
    }
}

#[tokio::test]
async fn test_stream_garbage_collection() {
    // Create a transaction stream
    let streaming_service_config = DataStreamingServiceConfig::default();
    let (mut data_stream, mut stream_listener) = create_transaction_stream(
        streaming_service_config,
        MIN_ADVERTISED_TRANSACTION_OUTPUT,
        MAX_ADVERTISED_TRANSACTION_OUTPUT,
    );

    // Initialize the data stream
    let global_data_summary = create_global_data_summary(1);
    data_stream
        .initialize_data_requests(global_data_summary.clone())
        .unwrap();

    loop {
        // Insert a transaction response into the queue
        set_transaction_response_at_queue_head(&mut data_stream);

        // Process the data response
        data_stream
            .process_data_responses(global_data_summary.clone())
            .unwrap();

        // Process the data response
        let data_notification = get_data_notification(&mut stream_listener).await.unwrap();
        if matches!(data_notification.data_payload, DataPayload::EndOfStream) {
            return;
        }

        // Verify the notification to response map is garbage collected
        let (_, sent_notifications) = data_stream.get_sent_requests_and_notifications();
        assert!(
            (sent_notifications.len() as u64)
                <= streaming_service_config.max_notification_id_mappings
        );
    }
}

#[tokio::test]
async fn test_stream_initialization() {
    // Create an epoch ending data stream
    let streaming_service_config = DataStreamingServiceConfig::default();
    let (mut data_stream, _) =
        create_epoch_ending_stream(streaming_service_config, MIN_ADVERTISED_EPOCH_END);

    // Verify the data stream is not initialized
    assert!(!data_stream.data_requests_initialized());

    // Initialize the data stream
    data_stream
        .initialize_data_requests(create_global_data_summary(100))
        .unwrap();

    // Verify the data stream is now initialized
    assert!(data_stream.data_requests_initialized());

    // Verify that client requests have been made
    let (sent_requests, _) = data_stream.get_sent_requests_and_notifications();
    assert_ne!(sent_requests.as_ref().unwrap().len(), 0);
}

#[tokio::test]
async fn test_stream_data_error() {
    // Create an epoch ending data stream
    let streaming_service_config = DataStreamingServiceConfig::default();
    let (mut data_stream, mut stream_listener) =
        create_epoch_ending_stream(streaming_service_config, MIN_ADVERTISED_EPOCH_END);

    // Initialize the data stream
    let global_data_summary = create_global_data_summary(100);
    data_stream
        .initialize_data_requests(global_data_summary.clone())
        .unwrap();

    // Clear the pending queue and insert an error response
    let client_request = DataClientRequest::EpochEndingLedgerInfos(EpochEndingLedgerInfosRequest {
        start_epoch: MIN_ADVERTISED_EPOCH_END,
        end_epoch: MIN_ADVERTISED_EPOCH_END + 1,
    });
    let pending_response = PendingClientResponse {
        client_request: client_request.clone(),
        client_response: Some(Err(aptos_data_client::Error::DataIsUnavailable(
            "Missing data!".into(),
        ))),
    };
    insert_response_into_pending_queue(&mut data_stream, pending_response);

    // Process the responses and verify the data client request was resent to the network
    data_stream
        .process_data_responses(global_data_summary)
        .unwrap();
    assert_none!(stream_listener.select_next_some().now_or_never());
    verify_client_request_resubmitted(&mut data_stream, client_request);
}

#[tokio::test]
async fn test_stream_invalid_response() {
    // Create an epoch ending data stream
    let streaming_service_config = DataStreamingServiceConfig::default();
    let (mut data_stream, mut stream_listener) =
        create_epoch_ending_stream(streaming_service_config, MIN_ADVERTISED_EPOCH_END);

    // Initialize the data stream
    let global_data_summary = create_global_data_summary(100);
    data_stream
        .initialize_data_requests(global_data_summary.clone())
        .unwrap();

    // Clear the pending queue and insert a response with an invalid type
    let client_request = DataClientRequest::EpochEndingLedgerInfos(EpochEndingLedgerInfosRequest {
        start_epoch: MIN_ADVERTISED_EPOCH_END,
        end_epoch: MIN_ADVERTISED_EPOCH_END + 1,
    });
    let context = ResponseContext {
        id: 0,
        response_callback: Box::new(NoopResponseCallback),
    };
    let client_response = Response::new(context, ResponsePayload::NumberOfStates(10));
    let pending_response = PendingClientResponse {
        client_request: client_request.clone(),
        client_response: Some(Ok(client_response)),
    };
    insert_response_into_pending_queue(&mut data_stream, pending_response);

    // Process the responses and verify the data client request was resent to the network
    data_stream
        .process_data_responses(global_data_summary)
        .unwrap();
    assert_none!(stream_listener.select_next_some().now_or_never());
    verify_client_request_resubmitted(&mut data_stream, client_request);
}

#[tokio::test]
async fn test_epoch_stream_out_of_order_responses() {
    // Create an epoch ending data stream
    let max_concurrent_requests = 3;
    let streaming_service_config = DataStreamingServiceConfig {
        max_concurrent_requests,
        max_concurrent_state_requests: 1,
        ..Default::default()
    };
    let (mut data_stream, mut stream_listener) =
        create_epoch_ending_stream(streaming_service_config, MIN_ADVERTISED_EPOCH_END);

    // Initialize the data stream
    let global_data_summary = create_global_data_summary(1);
    data_stream
        .initialize_data_requests(global_data_summary.clone())
        .unwrap();

    // Verify at least three requests have been made
    let (sent_requests, _) = data_stream.get_sent_requests_and_notifications();
    assert_ge!(
        sent_requests.as_ref().unwrap().len(),
        max_concurrent_requests as usize
    );

    // Set a response for the second request and verify no notifications
    set_epoch_ending_response_in_queue(&mut data_stream, 1);
    data_stream
        .process_data_responses(global_data_summary.clone())
        .unwrap();
    assert_none!(stream_listener.select_next_some().now_or_never());

    // Set a response for the first request and verify two notifications
    set_epoch_ending_response_in_queue(&mut data_stream, 0);
    data_stream
        .process_data_responses(global_data_summary.clone())
        .unwrap();
    for _ in 0..2 {
        verify_epoch_ending_notification(
            &mut stream_listener,
            create_ledger_info(0, MIN_ADVERTISED_EPOCH_END, true),
        )
        .await;
    }
    assert_none!(stream_listener.select_next_some().now_or_never());

    // Set the response for the first and third request and verify one notification sent
    set_epoch_ending_response_in_queue(&mut data_stream, 0);
    set_epoch_ending_response_in_queue(&mut data_stream, 2);
    data_stream
        .process_data_responses(global_data_summary.clone())
        .unwrap();
    verify_epoch_ending_notification(
        &mut stream_listener,
        create_ledger_info(0, MIN_ADVERTISED_EPOCH_END, true),
    )
    .await;
    assert_none!(stream_listener.select_next_some().now_or_never());

    // Set the response for the first and third request and verify three notifications sent
    set_epoch_ending_response_in_queue(&mut data_stream, 0);
    set_epoch_ending_response_in_queue(&mut data_stream, 2);
    data_stream
        .process_data_responses(global_data_summary.clone())
        .unwrap();
    for _ in 0..3 {
        verify_epoch_ending_notification(
            &mut stream_listener,
            create_ledger_info(0, MIN_ADVERTISED_EPOCH_END, true),
        )
        .await;
    }
    assert_none!(stream_listener.select_next_some().now_or_never());
}

#[tokio::test]
async fn test_state_stream_out_of_order_responses() {
    // Create a state value data stream
    let max_concurrent_state_requests = 6;
    let streaming_service_config = DataStreamingServiceConfig {
        max_concurrent_requests: 1,
        max_concurrent_state_requests,
        ..Default::default()
    };
    let (mut data_stream, mut stream_listener) =
        create_state_value_stream(streaming_service_config, MIN_ADVERTISED_STATES);

    // Initialize the data stream
    let global_data_summary = create_global_data_summary(1);
    data_stream
        .initialize_data_requests(global_data_summary.clone())
        .unwrap();

    // Verify a single request is made (to fetch the number of state values)
    let (sent_requests, _) = data_stream.get_sent_requests_and_notifications();
    assert_eq!(sent_requests.as_ref().unwrap().len(), 1);

    // Set a response for the number of state values
    set_num_state_values_response_in_queue(&mut data_stream, 0);
    data_stream
        .process_data_responses(global_data_summary.clone())
        .unwrap();

    // Verify at least six requests have been made
    let (sent_requests, _) = data_stream.get_sent_requests_and_notifications();
    assert_ge!(
        sent_requests.as_ref().unwrap().len(),
        max_concurrent_state_requests as usize
    );

    // Set a response for the second request and verify no notifications
    set_state_value_response_in_queue(&mut data_stream, 1);
    data_stream
        .process_data_responses(global_data_summary.clone())
        .unwrap();
    assert_none!(stream_listener.select_next_some().now_or_never());

    // Set a response for the first request and verify two notifications
    set_state_value_response_in_queue(&mut data_stream, 0);
    data_stream
        .process_data_responses(global_data_summary.clone())
        .unwrap();
    for _ in 0..2 {
        let data_notification = get_data_notification(&mut stream_listener).await.unwrap();
        assert_matches!(
            data_notification.data_payload,
            DataPayload::StateValuesWithProof(_)
        );
    }
    assert_none!(stream_listener.select_next_some().now_or_never());

    // Set the response for the first and third request and verify one notification sent
    set_state_value_response_in_queue(&mut data_stream, 0);
    set_state_value_response_in_queue(&mut data_stream, 2);
    data_stream
        .process_data_responses(global_data_summary.clone())
        .unwrap();
    let data_notification = get_data_notification(&mut stream_listener).await.unwrap();
    assert_matches!(
        data_notification.data_payload,
        DataPayload::StateValuesWithProof(_)
    );
    assert_none!(stream_listener.select_next_some().now_or_never());

    // Set the response for the first and third request and verify three notifications sent
    set_state_value_response_in_queue(&mut data_stream, 0);
    set_state_value_response_in_queue(&mut data_stream, 2);
    data_stream
        .process_data_responses(global_data_summary.clone())
        .unwrap();
    for _ in 0..3 {
        let data_notification = get_data_notification(&mut stream_listener).await.unwrap();
        assert_matches!(
            data_notification.data_payload,
            DataPayload::StateValuesWithProof(_)
        );
    }
    assert_none!(stream_listener.select_next_some().now_or_never());
}

#[tokio::test]
async fn test_stream_listener_dropped() {
    // Create an epoch ending data stream
    let max_concurrent_requests = 3;
    let streaming_service_config = DataStreamingServiceConfig {
        max_concurrent_requests,
        ..Default::default()
    };
    let (mut data_stream, mut stream_listener) =
        create_epoch_ending_stream(streaming_service_config, MIN_ADVERTISED_EPOCH_END);

    // Initialize the data stream
    let global_data_summary = create_global_data_summary(1);
    data_stream
        .initialize_data_requests(global_data_summary.clone())
        .unwrap();

    // Verify no notifications have been sent yet
    let (sent_requests, sent_notifications) = data_stream.get_sent_requests_and_notifications();
    assert_ge!(
        sent_requests.as_ref().unwrap().len(),
        max_concurrent_requests as usize
    );
    assert_eq!(sent_notifications.len(), 0);

    // Set a response for the first request and verify a notification is sent
    set_epoch_ending_response_in_queue(&mut data_stream, 0);
    data_stream
        .process_data_responses(global_data_summary.clone())
        .unwrap();
    verify_epoch_ending_notification(
        &mut stream_listener,
        create_ledger_info(0, MIN_ADVERTISED_EPOCH_END, true),
    )
    .await;

    // Verify a single notification was sent
    let (_, sent_notifications) = data_stream.get_sent_requests_and_notifications();
    assert_eq!(sent_notifications.len(), 1);

    // Drop the listener
    drop(stream_listener);

    // Set a response for the first request and verify an error is returned
    // when the notification is sent.
    set_epoch_ending_response_in_queue(&mut data_stream, 0);
    data_stream
        .process_data_responses(global_data_summary.clone())
        .unwrap_err();
    let (_, sent_notifications) = data_stream.get_sent_requests_and_notifications();
    assert_eq!(sent_notifications.len(), 2);

    // Set a response for the first request and verify no notifications are sent
    set_epoch_ending_response_in_queue(&mut data_stream, 0);
    data_stream
        .process_data_responses(global_data_summary.clone())
        .unwrap();
    let (_, sent_notifications) = data_stream.get_sent_requests_and_notifications();
    assert_eq!(sent_notifications.len(), 2);
}

/// Creates a state value stream for the given `version`.
fn create_state_value_stream(
    streaming_service_config: DataStreamingServiceConfig,
    version: Version,
) -> (DataStream<MockAptosDataClient>, DataStreamListener) {
    // Create a state value stream request
    let stream_request = StreamRequest::GetAllStates(GetAllStatesRequest {
        version,
        start_index: 0,
    });
    create_data_stream(streaming_service_config, stream_request)
}

/// Creates an epoch ending stream starting at `start_epoch`
fn create_epoch_ending_stream(
    streaming_service_config: DataStreamingServiceConfig,
    start_epoch: u64,
) -> (DataStream<MockAptosDataClient>, DataStreamListener) {
    // Create an epoch ending stream request
    let stream_request =
        StreamRequest::GetAllEpochEndingLedgerInfos(GetAllEpochEndingLedgerInfosRequest {
            start_epoch,
        });
    create_data_stream(streaming_service_config, stream_request)
}

/// Creates a transaction output stream for the given `version`.
fn create_transaction_stream(
    streaming_service_config: DataStreamingServiceConfig,
    start_version: Version,
    end_version: Version,
) -> (DataStream<MockAptosDataClient>, DataStreamListener) {
    // Create a transaction output stream
    let stream_request = StreamRequest::GetAllTransactions(GetAllTransactionsRequest {
        start_version,
        end_version,
        proof_version: end_version,
        include_events: false,
    });
    create_data_stream(streaming_service_config, stream_request)
}

fn create_data_stream(
    streaming_service_config: DataStreamingServiceConfig,
    stream_request: StreamRequest,
) -> (DataStream<MockAptosDataClient>, DataStreamListener) {
    initialize_logger();

    // Create an advertised data
    let advertised_data = AdvertisedData {
        states: vec![CompleteDataRange::new(MIN_ADVERTISED_STATES, MAX_ADVERTISED_STATES).unwrap()],
        epoch_ending_ledger_infos: vec![CompleteDataRange::new(
            MIN_ADVERTISED_EPOCH_END,
            MAX_ADVERTISED_EPOCH_END,
        )
        .unwrap()],
        synced_ledger_infos: vec![],
        transactions: vec![],
        transaction_outputs: vec![CompleteDataRange::new(
            MIN_ADVERTISED_TRANSACTION_OUTPUT,
            MAX_ADVERTISED_TRANSACTION_OUTPUT,
        )
        .unwrap()],
    };

    // Create an aptos data client mock and notification generator
    let aptos_data_client = MockAptosDataClient::new(false, false, false);
    let notification_generator = Arc::new(U64IdGenerator::new());

    // Return the data stream and listener pair
    DataStream::new(
        streaming_service_config,
        create_random_u64(10000),
        &stream_request,
        aptos_data_client,
        notification_generator,
        &advertised_data,
    )
    .unwrap()
}

fn create_global_data_summary(chunk_sizes: u64) -> GlobalDataSummary {
    let mut global_data_summary = GlobalDataSummary::empty();
    global_data_summary.optimal_chunk_sizes = create_optimal_chunk_sizes(chunk_sizes);
    global_data_summary
}

fn create_optimal_chunk_sizes(chunk_sizes: u64) -> OptimalChunkSizes {
    OptimalChunkSizes {
        state_chunk_size: chunk_sizes,
        epoch_chunk_size: chunk_sizes,
        transaction_chunk_size: chunk_sizes,
        transaction_output_chunk_size: chunk_sizes,
    }
}

/// Sets the client response at the index in the pending queue to contain an
/// epoch ending data response.
fn set_epoch_ending_response_in_queue(
    data_stream: &mut DataStream<MockAptosDataClient>,
    index: usize,
) {
    // Set the response at the specified index
    let (sent_requests, _) = data_stream.get_sent_requests_and_notifications();
    let pending_response = sent_requests.as_mut().unwrap().get_mut(index).unwrap();
    let client_response = Some(Ok(create_data_client_response(
        ResponsePayload::EpochEndingLedgerInfos(vec![create_ledger_info(
            0,
            MIN_ADVERTISED_EPOCH_END,
            true,
        )]),
    )));
    pending_response.lock().client_response = client_response;
}

/// Sets the client response at the index in the pending queue to contain a
/// number of state values response.
fn set_num_state_values_response_in_queue(
    data_stream: &mut DataStream<MockAptosDataClient>,
    index: usize,
) {
    let (sent_requests, _) = data_stream.get_sent_requests_and_notifications();
    let pending_response = sent_requests.as_mut().unwrap().get_mut(index).unwrap();
    let client_response = Some(Ok(create_data_client_response(
        ResponsePayload::NumberOfStates(1000000),
    )));
    pending_response.lock().client_response = client_response;
}

/// Sets the client response at the index in the pending queue to contain an
/// state value data response.
fn set_state_value_response_in_queue(
    data_stream: &mut DataStream<MockAptosDataClient>,
    index: usize,
) {
    let (sent_requests, _) = data_stream.get_sent_requests_and_notifications();
    let pending_response = sent_requests.as_mut().unwrap().get_mut(index).unwrap();
    let client_response = Some(Ok(create_data_client_response(
        ResponsePayload::StateValuesWithProof(StateValueChunkWithProof {
            first_index: 0,
            last_index: 0,
            first_key: Default::default(),
            last_key: Default::default(),
            raw_values: vec![],
            proof: SparseMerkleRangeProof::new(vec![]),
            root_hash: Default::default(),
        }),
    )));
    pending_response.lock().client_response = client_response;
}

/// Sets the client response at the head of the pending queue to contain an
/// transaction response.
fn set_transaction_response_at_queue_head(data_stream: &mut DataStream<MockAptosDataClient>) {
    // Set the response at the specified index
    let (sent_requests, _) = data_stream.get_sent_requests_and_notifications();
    if !sent_requests.as_mut().unwrap().is_empty() {
        let pending_response = sent_requests.as_mut().unwrap().get_mut(0).unwrap();
        let client_response = Some(Ok(create_data_client_response(
            ResponsePayload::TransactionsWithProof(create_transaction_list_with_proof(0, 0, false)),
        )));
        pending_response.lock().client_response = client_response;
    }
}

/// Clears the pending queue of the given data stream and inserts a single
/// response into the head of the queue.
fn insert_response_into_pending_queue(
    data_stream: &mut DataStream<MockAptosDataClient>,
    pending_response: PendingClientResponse,
) {
    // Clear the queue
    let (sent_requests, _) = data_stream.get_sent_requests_and_notifications();
    sent_requests.as_mut().unwrap().clear();

    // Insert the pending response
    let pending_response = Arc::new(Mutex::new(Box::new(pending_response)));
    sent_requests.as_mut().unwrap().push_front(pending_response);
}

/// Verifies that a client request was resubmitted (i.e., pushed to the head of the
/// sent request queue)
fn verify_client_request_resubmitted(
    data_stream: &mut DataStream<MockAptosDataClient>,
    client_request: DataClientRequest,
) {
    let (sent_requests, _) = data_stream.get_sent_requests_and_notifications();
    let pending_response = sent_requests.as_mut().unwrap().pop_front().unwrap();
    assert_eq!(pending_response.lock().client_request, client_request);
    assert_none!(pending_response.lock().client_response.as_ref());
}

/// Verifies that a single epoch ending notification is received by the
/// data listener and that it contains the `expected_ledger_info`.
async fn verify_epoch_ending_notification(
    stream_listener: &mut DataStreamListener,
    expected_ledger_info: LedgerInfoWithSignatures,
) {
    let data_notification = get_data_notification(stream_listener).await.unwrap();
    if let DataPayload::EpochEndingLedgerInfos(ledger_infos) = data_notification.data_payload {
        assert_eq!(ledger_infos[0], expected_ledger_info);
    } else {
        panic!(
            "Expected an epoch ending ledger info payload, but got: {:?}",
            data_notification
        );
    }
}
