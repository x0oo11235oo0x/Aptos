# frozen_string_literal: true

# Copyright (c) Aptos
# SPDX-License-Identifier: Apache-2.0

class DiscourseAddGroupJob < DiscourseJob
  # Ex args: { user_id: 32, group_name: "group-name" }
  def perform(args)
    @args = args
    @user = User.find(args[:user_id])
    @group_name = args[:group_name]

    unless @user.registration_completed?
      return Rails.logger.debug("User not confirmed: #{@user.id} - #{@user.external_id}")
    end

    @client = DiscourseHelper.system_client

    add_group
  end

  def add_group
    return if discourse_user_id.nil? || group_id.nil?

    @client.group_add(group_id, user_id: discourse_user_id)
  rescue DiscourseApi::UnprocessableEntity => e
    return if e.response.body['errors'].first.to_s.include? 'already a member of this group'

    raise
  end

  memoize def group_id
    group = @client.group(@group_name)
    id = group['group']['id']
    Rails.logger.debug("Fetched forum group id #{id} for group #{@group_name}")
    id
  end
end

# DiscourseAddGroupJob.perform_now({ user_id: 4, group_name: "ait3_eligible" })
